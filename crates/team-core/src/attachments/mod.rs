//! T-32 attachment policy layer.
//!
//! The agent receives a message body containing `📎 attachment: <path>`
//! and asks the broker to read the file. This module owns the
//! decision: accept (return bytes) or reject (return reason). It is
//! transport-agnostic — the MCP `read_attachment` tool is one caller;
//! a future REST surface or CLI debug helper would call the same
//! `check_and_read` entry point.
//!
//! Three independent guards layered before any read:
//! 1. **Path-traversal**: canonicalize the operator-supplied path and
//!    confirm it is a descendant of one of `allowed_roots`.
//! 2. **Size**: validate an opened regular file and read at most cap + 1.
//! 3. **Scanner** (optional): scan a private copy of the captured bytes with
//!    a timeout, killing and reaping failed/timed-out scanner processes.
//!
//! Bytes are returned as-is on accept. No envelope wrapping, no
//! "treat as data" framing — those are prompt-injection mitigations
//! and live in the hook layer per owner ratify.
//!
//! `enabled = false` short-circuits with `RejectReason::Disabled` —
//! no filesystem cost when the operator has flipped the flag.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::compose::{AttachmentScanner, Attachments};

/// Reasons the broker can refuse to read an attachment. The agent
/// surfaces the variant + a short string back to the operator via
/// the originating-channel notification path; the operator never
/// sees raw filesystem errors verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Operator set `attachments.enabled: false`. Agent should treat
    /// the request as if attachments were unsupported.
    Disabled,
    /// `allowed_roots` was empty after `$HOME` expansion. The
    /// operator misconfigured — no path can ever resolve.
    NoAllowedRoots,
    /// Path failed to canonicalize (does not exist, broken symlink,
    /// permission denied during traversal). Carries the OS-level
    /// reason for operator-side debugging.
    PathUnresolvable(String),
    /// Resolved path is not a descendant of any `allowed_root`.
    OutsideAllowedRoots { resolved: PathBuf },
    /// Resolved path stat'd > `max_size_bytes`.
    TooLarge { size: u64, cap: u64 },
    /// Scanner subprocess returned non-zero, timed out, or could not
    /// be spawned. `detail` carries the scanner's stderr (truncated)
    /// or a wrapper-level error.
    ScannerRejected { detail: String },
    /// Compose configured a scanner but the caller passed
    /// `scanner: None` to `check_and_read`. Tighter than silently
    /// skipping the scan: a refactor that drops the scanner arg
    /// anywhere upstream surfaces here instead of disabling
    /// malware checking with no test failure.
    ScannerNotProvided,
    /// Read raced with deletion or another `fs::read` failure surfaced
    /// after the size check passed.
    ReadFailed(String),
}

impl RejectReason {
    /// One-liner suitable for inclusion in an operator-facing
    /// notification. Avoids markdown chars so HTML / plain renderers
    /// both reproduce it byte-for-byte (T-134 coordination with
    /// wren — no `_*<>&` that would need escaping in either path).
    pub fn human(&self) -> String {
        match self {
            Self::Disabled => "attachments are disabled in this team's compose".into(),
            Self::NoAllowedRoots => {
                "no allowed_roots resolved — check attachments.allowed_roots config".into()
            }
            Self::PathUnresolvable(e) => format!("could not resolve path: {e}"),
            Self::OutsideAllowedRoots { resolved } => format!(
                "path resolves outside allowed_roots: {}",
                resolved.display()
            ),
            Self::TooLarge { size, cap } => {
                format!("file size {size} bytes exceeds the {cap}-byte cap")
            }
            Self::ScannerRejected { detail } => format!("scanner rejected: {detail}"),
            Self::ScannerNotProvided => {
                "scanner is configured but the broker did not run it (internal misconfiguration)"
                    .into()
            }
            Self::ReadFailed(e) => format!("read failed: {e}"),
        }
    }
}

/// External-scanner abstraction. Implementations spawn the operator's
/// configured command, wait up to `timeout`, and return the outcome.
/// Trait-object shape keeps the read path testable without spawning
/// real processes — the test seam is the Mock impl in `#[cfg(test)]`.
pub trait Scanner: Send + Sync {
    fn scan(&self, path: &Path, timeout: Duration) -> ScanOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanOutcome {
    Clean,
    Rejected { detail: String },
}

/// Resolve `$HOME` and other allow-list roots to canonical paths.
/// Performed at check-time so a snapshot taken on machine A still
/// resolves correctly when restored on machine B (different `$HOME`).
/// Roots that fail to canonicalize are dropped — an operator with a
/// stale path entry doesn't break the whole policy.
pub fn resolve_allowed_roots(cfg: &Attachments) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    cfg.allowed_roots
        .iter()
        .map(|s| s.as_str())
        .filter_map(|spec| {
            let raw = if spec == "$HOME" {
                home.clone()?
            } else if let Some(rest) = spec.strip_prefix("$HOME/") {
                home.clone().map(|h| h.join(rest))?
            } else {
                PathBuf::from(spec)
            };
            raw.canonicalize().ok()
        })
        .collect()
}

/// Pure check: is `resolved` a descendant of (or equal to) any of
/// `roots`? Both sides are expected canonical, so byte-equality
/// `starts_with` is enough — no `..` slipping through.
pub fn is_within_any_root(resolved: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| resolved.starts_with(r))
}

/// Attempt to read the file the operator pointed at, applying every
/// configured guard. The scanner is plumbed through as a trait object
/// so callers (production: real `Command`; tests: mock) share the
/// same control flow.
pub fn check_and_read(
    cfg: &Attachments,
    raw_path: &Path,
    scanner: Option<&dyn Scanner>,
) -> Result<Vec<u8>, RejectReason> {
    read_checked(cfg, raw_path, scanner).map(|(bytes, _)| bytes)
}

/// Validate the opened regular file, capture bounded bytes once, then scan a
/// private snapshot of those same bytes. The source path is never reopened
/// after scanning and audit metadata refers to the validated read.
fn read_checked(
    cfg: &Attachments,
    raw_path: &Path,
    scanner: Option<&dyn Scanner>,
) -> Result<(Vec<u8>, PathBuf), RejectReason> {
    if !cfg.enabled {
        return Err(RejectReason::Disabled);
    }
    let roots = resolve_allowed_roots(cfg);
    if roots.is_empty() {
        return Err(RejectReason::NoAllowedRoots);
    }
    let resolved = raw_path
        .canonicalize()
        .map_err(|e| RejectReason::PathUnresolvable(e.to_string()))?;
    if !is_within_any_root(&resolved, &roots) {
        return Err(RejectReason::OutsideAllowedRoots { resolved });
    }
    let file = open_regular(&resolved).map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
    // Ask the kernel for the opened descriptor's path. A second pathname
    // lookup can race a parent-directory replacement independently of open.
    let opened_path =
        descriptor_path(&file, &resolved).map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
    if !is_within_any_root(&opened_path, &roots) {
        return Err(RejectReason::OutsideAllowedRoots {
            resolved: opened_path,
        });
    }
    if metadata.len() > cfg.max_size_bytes {
        return Err(RejectReason::TooLarge {
            size: metadata.len(),
            cap: cfg.max_size_bytes,
        });
    }
    let bytes = read_bounded(file, cfg.max_size_bytes)?;
    if let Some(spec) = cfg.scanner.as_ref() {
        let Some(s) = scanner else {
            return Err(RejectReason::ScannerNotProvided);
        };
        // Scanners see an immutable private copy with the original suffix.
        // This pins the accepted content even if the source changes mid-scan.
        let dir = tempfile::tempdir().map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
        let suffix = opened_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_default();
        let mut snapshot = tempfile::Builder::new()
            .suffix(&suffix)
            .tempfile_in(dir.path())
            .map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
        snapshot
            .write_all(&bytes)
            .map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
        snapshot
            .flush()
            .map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
        if let ScanOutcome::Rejected { detail } =
            s.scan(snapshot.path(), Duration::from_secs(spec.timeout_seconds))
        {
            return Err(RejectReason::ScannerRejected { detail });
        }
        // Detect a misbehaving scanner that rewrites its input rather than
        // certifying it, while still returning the original captured bytes.
        let scanned = read_bounded(
            open_regular(snapshot.path()).map_err(|e| RejectReason::ReadFailed(e.to_string()))?,
            cfg.max_size_bytes,
        )?;
        if scanned != bytes {
            return Err(RejectReason::ScannerRejected {
                detail: "scanner modified attachment".into(),
            });
        }
    }
    Ok((bytes, opened_path))
}

/// Validate the opened handle, so a path replacement cannot introduce a
/// blocking FIFO or a final symlink between stat and open.
fn open_regular(path: &Path) -> std::io::Result<fs::File> {
    open_regular_with_attributes(path, false)
}

fn open_regular_with_attributes(path: &Path, _write_attributes: bool) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    if _write_attributes {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_WRITE_ATTRIBUTES};
        options.access_mode(FILE_GENERIC_READ | FILE_WRITE_ATTRIBUTES);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "attachment is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn descriptor_path(file: &fs::File, _resolved: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(target_os = "macos")]
fn descriptor_path(file: &fs::File, _resolved: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: the live file owns fd; F_GETPATH writes a NUL-terminated path
    // into a buffer of PATH_MAX bytes, as required by the macOS API.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, bytes.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let end = bytes.iter().position(|b| *b == 0).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "descriptor path missing terminator",
        )
    })?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

#[cfg(windows)]
fn descriptor_path(file: &fs::File, _resolved: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 32768];
    // SAFETY: file owns a live handle and buffer has the declared capacity.
    let size = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if size == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if size as usize >= buffer.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "descriptor path too long",
        ));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..size as usize],
    )))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn descriptor_path(_file: &fs::File, _resolved: &Path) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "secure attachment path validation is unsupported on this platform",
    ))
}

fn read_bounded(reader: impl Read, cap: u64) -> Result<Vec<u8>, RejectReason> {
    let mut bytes = Vec::new();
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
    if bytes.len() as u64 > cap {
        return Err(RejectReason::TooLarge {
            size: bytes.len() as u64,
            cap,
        });
    }
    Ok(bytes)
}

/// T-32b: outcome of a successful read — bytes plus metadata the
/// caller surfaces to the agent (and writes to the audit log).
#[derive(Debug, Clone)]
pub struct AcceptedAttachment {
    pub bytes: Vec<u8>,
    pub blake3_hex: String,
    pub size: u64,
    /// Canonicalized path that passed the policy check. Returned so
    /// the audit log captures the resolved-not-the-typed path.
    pub resolved: PathBuf,
}

/// Wrapper that runs `check_and_read` and packages the result with
/// the metadata team-mcp's `read_attachment` tool returns to the
/// agent. Centralises hashing so the staging-tempfile name (content-
/// addressed) and the audit log entry stay in sync.
pub fn check_and_read_with_metadata(
    cfg: &Attachments,
    raw_path: &Path,
    scanner: Option<&dyn Scanner>,
) -> Result<AcceptedAttachment, RejectReason> {
    let (bytes, resolved) = read_checked(cfg, raw_path, scanner)?;
    let blake3_hex = blake3::hash(&bytes).to_hex().to_string();
    let size = bytes.len() as u64;
    Ok(AcceptedAttachment {
        bytes,
        blake3_hex,
        size,
        resolved,
    })
}

mod audit;
mod scanner;
mod staging;

pub use audit::{append_audit, now_rfc3339, AuditEntry};
pub use scanner::RealScanner;
pub use staging::{stage_to_tempfile, staging_dir, sweep_expired};

#[cfg(test)]
mod tests;
