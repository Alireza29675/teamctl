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
//! 2. **Size**: stat the file and reject if it exceeds `max_size_bytes`
//!    before any bytes are read.
//! 3. **Scanner** (optional): hand the canonical path to an external
//!    command with a timeout; non-zero exit or timeout → reject.
//!
//! Bytes are returned as-is on accept. No envelope wrapping, no
//! "treat as data" framing — those are prompt-injection mitigations
//! and live in the hook layer per owner ratify.
//!
//! `enabled = false` short-circuits with `RejectReason::Disabled` —
//! no filesystem cost when the operator has flipped the flag.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

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
    let metadata = fs::metadata(&resolved).map_err(|e| RejectReason::ReadFailed(e.to_string()))?;
    if metadata.len() > cfg.max_size_bytes {
        return Err(RejectReason::TooLarge {
            size: metadata.len(),
            cap: cfg.max_size_bytes,
        });
    }
    // Tight scanner contract: if the operator configured a scanner,
    // the caller MUST hand one to `check_and_read`. A `None` here
    // surfaces as `ScannerNotProvided` rather than silently skipping
    // the scan — caught by the unit test below, so a refactor that
    // drops the arg upstream fails loudly instead of disabling
    // malware checking.
    if let Some(spec) = cfg.scanner.as_ref() {
        let Some(s) = scanner else {
            return Err(RejectReason::ScannerNotProvided);
        };
        let outcome = s.scan(&resolved, Duration::from_secs(spec.timeout_seconds));
        if let ScanOutcome::Rejected { detail } = outcome {
            return Err(RejectReason::ScannerRejected { detail });
        }
    }
    fs::read(&resolved).map_err(|e| RejectReason::ReadFailed(e.to_string()))
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
    let bytes = check_and_read(cfg, raw_path, scanner)?;
    let blake3_hex = blake3::hash(&bytes).to_hex().to_string();
    let resolved = raw_path
        .canonicalize()
        .map_err(|e| RejectReason::PathUnresolvable(e.to_string()))?;
    let size = bytes.len() as u64;
    Ok(AcceptedAttachment {
        bytes,
        blake3_hex,
        size,
        resolved,
    })
}

/// T-32b staging directory under the compose root. Tempfiles live
/// here, named by the content blake3 hash so identical content
/// dedups to a single file across sessions and across agents.
pub fn staging_dir(compose_root: &Path) -> PathBuf {
    compose_root.join("state/attachments-staging")
}

/// Write `accepted.bytes` to the staging dir under a content-
/// addressed name. Idempotent: if the file already exists with the
/// expected size, we skip the write (operator may have read the
/// same attachment recently). Returns the staged path so the agent
/// can `read_file()` it directly.
pub fn stage_to_tempfile(
    staging_dir: &Path,
    accepted: &AcceptedAttachment,
) -> Result<PathBuf, std::io::Error> {
    fs::create_dir_all(staging_dir)?;
    let path = staging_dir.join(&accepted.blake3_hex);
    let needs_write = !matches!(fs::metadata(&path), Ok(m) if m.len() == accepted.size);
    if needs_write {
        // Atomic write via tempfile + rename so a crash mid-write
        // can't leave a half-baked stage file with the canonical name.
        let tmp = staging_dir.join(format!("{}.tmp", &accepted.blake3_hex));
        fs::write(&tmp, &accepted.bytes)?;
        fs::rename(&tmp, &path)?;
    } else {
        // Bump mtime so the sweep doesn't reap a freshly-touched file.
        let _ = touch(&path);
    }
    Ok(path)
}

fn touch(path: &Path) -> std::io::Result<()> {
    let now = SystemTime::now();
    let f = fs::OpenOptions::new().write(true).open(path)?;
    f.set_modified(now)?;
    Ok(())
}

/// T-32b: drop tempfiles whose mtime is older than `now - ttl`.
/// Called on team-mcp startup as a best-effort cleanup. Returns the
/// number of files reaped so callers can log a single summary line.
/// Errors traversing individual entries are logged-and-skipped at
/// the call site (kept out of this function so unit tests stay
/// trace-free).
pub fn sweep_expired(staging_dir: &Path, ttl: Duration) -> std::io::Result<usize> {
    if !staging_dir.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(ttl)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut reaped = 0usize;
    for entry in fs::read_dir(staging_dir)? {
        let entry = entry?;
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if mtime < cutoff && fs::remove_file(entry.path()).is_ok() {
            reaped += 1;
        }
    }
    Ok(reaped)
}

/// Production scanner: spawns the operator-configured command with
/// the resolved path as a single argument, waits up to `timeout`,
/// captures stderr for the reject detail. The wait uses
/// `std::thread::spawn` + `mpsc::recv_timeout` so team-core stays
/// sync (no tokio dep — owner-ratify variant 4).
pub struct RealScanner;

impl Scanner for RealScanner {
    fn scan(&self, path: &Path, timeout: Duration) -> ScanOutcome {
        // Look up the configured command from the closure-captured
        // spec via the wrapper below — the trait API doesn't carry
        // the command, callers construct via `RealScanner::for_spec`.
        // Default impl: panic-free fallback for the trait when used
        // without the wrapper. In practice the wrapper is the only
        // caller (see `for_spec`).
        let _ = (path, timeout);
        ScanOutcome::Rejected {
            detail: "RealScanner used without a configured command".into(),
        }
    }
}

impl RealScanner {
    /// Bind a `RealScanner` to a specific scanner spec. Returns a
    /// boxed trait object so the call site stays uniform with the
    /// `MockScanner` shape used in tests.
    pub fn for_spec(spec: &AttachmentScanner) -> Box<dyn Scanner> {
        Box::new(RealScannerForSpec {
            command: spec.command.clone(),
        })
    }
}

struct RealScannerForSpec {
    command: String,
}

impl Scanner for RealScannerForSpec {
    fn scan(&self, path: &Path, timeout: Duration) -> ScanOutcome {
        let cmd = self.command.clone();
        let path_owned = path.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = Command::new(&cmd)
                .arg(&path_owned)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            let _ = tx.send(result);
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => {
                if output.status.success() {
                    ScanOutcome::Clean
                } else {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let detail = if detail.is_empty() {
                        format!("exited with status {}", output.status)
                    } else {
                        truncate_for_reject(&detail)
                    };
                    ScanOutcome::Rejected { detail }
                }
            }
            Ok(Err(e)) => ScanOutcome::Rejected {
                detail: format!("scanner spawn failed: {e}"),
            },
            Err(_) => ScanOutcome::Rejected {
                detail: format!("scanner timed out after {}s", timeout.as_secs()),
            },
        }
    }
}

/// Cap scanner stderr so a chatty scanner can't blow up the reject
/// notification's wire size or the audit log line. 512 chars is
/// plenty for diagnostic context (the operator's full scanner log
/// is on disk anyway).
fn truncate_for_reject(s: &str) -> String {
    const CAP: usize = 512;
    if s.chars().count() <= CAP {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(CAP).collect();
        out.push('…');
        out
    }
}

/// T-32b audit log entry. Written as a single JSON line per attempt
/// so the file is `tail -f`-friendly + parseable with `jq`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry<'a> {
    /// RFC3339 UTC timestamp.
    pub ts: String,
    /// Operator-supplied path verbatim — preserved so a typo is
    /// debuggable directly from the log.
    pub path: &'a str,
    /// Canonicalized path (`None` when the path failed to resolve).
    pub resolved: Option<&'a str>,
    pub outcome: &'static str,
    pub size: Option<u64>,
    pub blake3: Option<&'a str>,
    pub reason: Option<String>,
}

/// Append a single JSON-line audit entry. No-op when
/// `audit_log_path` is `None`. Errors creating the parent dir or
/// opening the file are surfaced to the caller — production paths
/// log-and-continue so a misconfigured audit dir doesn't block real
/// reads.
pub fn append_audit(audit_log_path: Option<&Path>, entry: &AuditEntry<'_>) -> std::io::Result<()> {
    let Some(p) = audit_log_path else {
        return Ok(());
    };
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    let mut f = fs::OpenOptions::new().append(true).create(true).open(p)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Helper: format an RFC3339 UTC timestamp suitable for audit
/// entries. Pulled out so tests can pin the format independently of
/// the call site.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Test scanner — accepts when configured to, rejects with a
    /// canned detail otherwise. The Mutex tracks which paths it was
    /// called on so tests can assert the scanner ran (or didn't).
    struct MockScanner {
        outcome: ScanOutcome,
        calls: Mutex<Vec<PathBuf>>,
    }

    impl MockScanner {
        fn clean() -> Self {
            Self {
                outcome: ScanOutcome::Clean,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn rejecting(detail: &str) -> Self {
            Self {
                outcome: ScanOutcome::Rejected {
                    detail: detail.into(),
                },
                calls: Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Scanner for MockScanner {
        fn scan(&self, path: &Path, _timeout: Duration) -> ScanOutcome {
            self.calls.lock().unwrap().push(path.to_path_buf());
            self.outcome.clone()
        }
    }

    fn cfg_with_root(root: &Path, max: u64) -> Attachments {
        Attachments {
            enabled: true,
            max_size_bytes: max,
            allowed_roots: vec![root.to_string_lossy().into_owned()],
            scanner: None,
            audit_log_path: None,
            tempfile_ttl_seconds: 6 * 60 * 60,
        }
    }

    #[test]
    fn disabled_short_circuits_before_any_filesystem_work() {
        // `enabled: false` returns Disabled even when the path would
        // pass every other check — the operator's flip is honoured
        // synchronously, no read attempted.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.txt");
        fs::write(&p, b"hi").unwrap();
        let mut cfg = cfg_with_root(dir.path(), 1024);
        cfg.enabled = false;
        assert_eq!(
            check_and_read(&cfg, &p, None).unwrap_err(),
            RejectReason::Disabled
        );
    }

    #[test]
    fn unresolvable_path_returns_path_unresolvable() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_root(dir.path(), 1024);
        let missing = dir.path().join("nope.txt");
        let err = check_and_read(&cfg, &missing, None).unwrap_err();
        assert!(
            matches!(err, RejectReason::PathUnresolvable(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn path_outside_allowed_roots_is_rejected() {
        // Two tempdirs: file lives in `outside`, allow-list points at
        // `inside`. Canonicalize on both sides so symlinks (macOS
        // /var → /private/var) don't false-positive.
        let inside = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let p = outside.path().join("leak.txt");
        fs::write(&p, b"x").unwrap();
        let cfg = cfg_with_root(inside.path(), 1024);
        let err = check_and_read(&cfg, &p, None).unwrap_err();
        assert!(
            matches!(err, RejectReason::OutsideAllowedRoots { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn file_above_size_cap_is_rejected_before_read() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("big.bin");
        fs::write(&p, vec![0u8; 16]).unwrap();
        let cfg = cfg_with_root(dir.path(), 8);
        let err = check_and_read(&cfg, &p, None).unwrap_err();
        match err {
            RejectReason::TooLarge { size, cap } => {
                assert_eq!(size, 16);
                assert_eq!(cap, 8);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn happy_path_returns_bytes_unmodified() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.md");
        fs::write(&p, b"hello attachments").unwrap();
        let cfg = cfg_with_root(dir.path(), 1024);
        let bytes = check_and_read(&cfg, &p, None).unwrap();
        assert_eq!(bytes, b"hello attachments");
    }

    #[test]
    fn scanner_clean_passes_through() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.md");
        fs::write(&p, b"hi").unwrap();
        let mut cfg = cfg_with_root(dir.path(), 1024);
        cfg.scanner = Some(crate::compose::AttachmentScanner {
            command: "true".into(),
            timeout_seconds: 30,
        });
        let scanner = MockScanner::clean();
        let bytes = check_and_read(&cfg, &p, Some(&scanner)).unwrap();
        assert_eq!(bytes, b"hi");
        assert_eq!(scanner.call_count(), 1, "scanner ran exactly once");
    }

    #[test]
    fn scanner_reject_blocks_read() {
        // Bytes must NOT come back — the agent never sees the file
        // content even though the size + path checks both passed.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("malware.exe");
        fs::write(&p, b"would-be-bad").unwrap();
        let mut cfg = cfg_with_root(dir.path(), 1024);
        cfg.scanner = Some(crate::compose::AttachmentScanner {
            command: "false".into(),
            timeout_seconds: 30,
        });
        let scanner = MockScanner::rejecting("EICAR test signature");
        let err = check_and_read(&cfg, &p, Some(&scanner)).unwrap_err();
        match err {
            RejectReason::ScannerRejected { detail } => {
                assert!(
                    detail.contains("EICAR"),
                    "scanner detail must surface to the reason: {detail}"
                );
            }
            other => panic!("expected ScannerRejected, got {other:?}"),
        }
    }

    #[test]
    fn scanner_only_runs_after_path_and_size_pass() {
        // Order matters for cost: an oversize file shouldn't pay the
        // scanner cost. Pin the order with a scanner that would
        // reject — if it ran, we'd see ScannerRejected; we expect
        // TooLarge instead.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("big.bin");
        fs::write(&p, vec![0u8; 100]).unwrap();
        let mut cfg = cfg_with_root(dir.path(), 8);
        cfg.scanner = Some(crate::compose::AttachmentScanner {
            command: "false".into(),
            timeout_seconds: 30,
        });
        let scanner = MockScanner::rejecting("would-reject");
        let err = check_and_read(&cfg, &p, Some(&scanner)).unwrap_err();
        assert!(matches!(err, RejectReason::TooLarge { .. }), "got {err:?}");
        assert_eq!(scanner.call_count(), 0, "scanner short-circuited");
    }

    #[test]
    fn human_message_avoids_markdown_chars() {
        // T-134 coordination: messages flow through team-bot's
        // render path. Both render_plain and the upcoming HTML
        // renderer must reproduce these byte-for-byte. Pinning a
        // representative sample.
        let r = RejectReason::TooLarge { size: 100, cap: 50 };
        let s = r.human();
        for c in ['<', '>', '&', '*', '_'] {
            assert!(!s.contains(c), "human() message contains `{c}`: {s}");
        }
    }

    #[test]
    fn empty_allowed_roots_returns_no_allowed_roots() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.txt");
        fs::write(&p, b"hi").unwrap();
        let mut cfg = cfg_with_root(dir.path(), 1024);
        cfg.allowed_roots = vec![];
        let err = check_and_read(&cfg, &p, None).unwrap_err();
        assert_eq!(err, RejectReason::NoAllowedRoots);
    }

    #[test]
    fn scanner_configured_but_caller_passes_none_returns_scanner_not_provided() {
        // Tight contract per peer review: a caller path that drops
        // the scanner argument while the compose still configures
        // one must surface as ScannerNotProvided. The previous shape
        // silently skipped the scan, which would let a refactor
        // disable malware checking with zero test failure.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.md");
        fs::write(&p, b"hi").unwrap();
        let mut cfg = cfg_with_root(dir.path(), 1024);
        cfg.scanner = Some(crate::compose::AttachmentScanner {
            command: "true".into(),
            timeout_seconds: 30,
        });
        let err = check_and_read(&cfg, &p, None).unwrap_err();
        assert_eq!(err, RejectReason::ScannerNotProvided);
    }

    #[test]
    fn is_within_any_root_handles_descendant_and_equal_paths() {
        let root = PathBuf::from("/tmp/team");
        let descendant = PathBuf::from("/tmp/team/sub/file.md");
        let elsewhere = PathBuf::from("/tmp/other/file.md");
        assert!(is_within_any_root(&descendant, std::slice::from_ref(&root)));
        assert!(is_within_any_root(&root, std::slice::from_ref(&root)));
        assert!(!is_within_any_root(&elsewhere, &[root]));
    }

    // ── T-32b: staging + sweep + audit + scanner ───────────────────

    fn accepted_for(bytes: &[u8]) -> AcceptedAttachment {
        AcceptedAttachment {
            bytes: bytes.to_vec(),
            blake3_hex: blake3::hash(bytes).to_hex().to_string(),
            size: bytes.len() as u64,
            resolved: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn stage_to_tempfile_writes_and_returns_content_addressed_path() {
        let dir = TempDir::new().unwrap();
        let staging = dir.path().join("attachments-staging");
        let accepted = accepted_for(b"hello attachments");
        let staged = stage_to_tempfile(&staging, &accepted).unwrap();
        assert!(staged.exists(), "staged file present: {}", staged.display());
        assert_eq!(fs::read(&staged).unwrap(), b"hello attachments");
        // Filename is the blake3 hex — content-addressed.
        assert_eq!(staged.file_name().unwrap(), accepted.blake3_hex.as_str());
    }

    #[test]
    fn stage_to_tempfile_is_idempotent_for_identical_content() {
        // Same content → same path → no overwrite. Operator reading
        // the same attachment twice in a session should not multiply
        // staging-dir size.
        let dir = TempDir::new().unwrap();
        let staging = dir.path().join("attachments-staging");
        let accepted = accepted_for(b"same bytes");
        let p1 = stage_to_tempfile(&staging, &accepted).unwrap();
        let p2 = stage_to_tempfile(&staging, &accepted).unwrap();
        assert_eq!(p1, p2);
        // Only one file in the staging dir.
        let entries: Vec<_> = fs::read_dir(&staging).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "no duplicates: {entries:?}");
    }

    #[test]
    fn sweep_expired_drops_stale_files_and_returns_count() {
        let dir = TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        // Two stage files: one freshly written, one with mtime
        // pushed back beyond the TTL.
        let fresh = staging.join("fresh");
        fs::write(&fresh, b"fresh").unwrap();
        let stale = staging.join("stale");
        fs::write(&stale, b"stale").unwrap();
        let old = SystemTime::now() - Duration::from_secs(7200);
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let reaped = sweep_expired(&staging, Duration::from_secs(3600)).unwrap();
        assert_eq!(reaped, 1);
        assert!(fresh.exists(), "fresh file kept");
        assert!(!stale.exists(), "stale file reaped");
    }

    #[test]
    fn sweep_expired_returns_zero_when_dir_missing() {
        // Best-effort cleanup — a missing staging dir on first boot
        // is normal, not an error.
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("never-created");
        let reaped = sweep_expired(&nonexistent, Duration::from_secs(60)).unwrap();
        assert_eq!(reaped, 0);
    }

    #[test]
    fn audit_no_op_when_path_unset() {
        // The configured-but-unset code path is the most common one
        // (default-no-audit); the helper has to be silent and
        // success-returning so callers don't need a separate guard.
        let entry = AuditEntry {
            ts: now_rfc3339(),
            path: "/whatever",
            resolved: None,
            outcome: "accept",
            size: Some(0),
            blake3: None,
            reason: None,
        };
        append_audit(None, &entry).unwrap();
    }

    #[test]
    fn audit_appends_jsonl_lines() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("audit.log");
        for i in 0..3 {
            let entry = AuditEntry {
                ts: format!("2026-05-10T15:00:0{i}Z"),
                path: "/some/path",
                resolved: Some("/canonical/path"),
                outcome: if i == 2 { "reject" } else { "accept" },
                size: Some(i),
                blake3: Some("abcdef"),
                reason: if i == 2 {
                    Some("too large".into())
                } else {
                    None
                },
            };
            append_audit(Some(&log), &entry).unwrap();
        }
        let body = fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3);
        // Each line is parseable JSON with the expected shape.
        for (i, line) in lines.iter().enumerate() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v["ts"].is_string());
            assert_eq!(v["size"].as_i64().unwrap(), i as i64);
        }
    }

    #[test]
    fn audit_creates_parent_dir_on_first_write() {
        // Operator points `audit_log_path` at a fresh subdir under
        // state/. The helper has to create the dir before opening
        // the file — no manual `mkdir -p` step.
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("audit/attempts.log");
        let entry = AuditEntry {
            ts: now_rfc3339(),
            path: "/x",
            resolved: None,
            outcome: "accept",
            size: None,
            blake3: None,
            reason: None,
        };
        append_audit(Some(&log), &entry).unwrap();
        assert!(log.exists());
    }

    #[test]
    fn check_and_read_with_metadata_returns_blake3_and_size() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ok.md");
        fs::write(&p, b"twelve chars").unwrap();
        let cfg = cfg_with_root(dir.path(), 1024);
        let acc = check_and_read_with_metadata(&cfg, &p, None).unwrap();
        assert_eq!(acc.size, 12);
        assert_eq!(acc.bytes, b"twelve chars");
        assert_eq!(
            acc.blake3_hex,
            blake3::hash(b"twelve chars").to_hex().to_string()
        );
    }

    #[test]
    fn real_scanner_for_spec_clean_path_returns_clean() {
        // `/usr/bin/true` — universally available on linux runners.
        if !Path::new("/usr/bin/true").exists() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let dummy = dir.path().join("any");
        fs::write(&dummy, b"x").unwrap();
        let scanner = RealScanner::for_spec(&AttachmentScanner {
            command: "/usr/bin/true".into(),
            timeout_seconds: 5,
        });
        assert_eq!(
            scanner.scan(&dummy, Duration::from_secs(5)),
            ScanOutcome::Clean
        );
    }

    #[test]
    fn real_scanner_for_spec_nonzero_exit_returns_rejected() {
        if !Path::new("/usr/bin/false").exists() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let dummy = dir.path().join("any");
        fs::write(&dummy, b"x").unwrap();
        let scanner = RealScanner::for_spec(&AttachmentScanner {
            command: "/usr/bin/false".into(),
            timeout_seconds: 5,
        });
        match scanner.scan(&dummy, Duration::from_secs(5)) {
            ScanOutcome::Rejected { detail } => {
                // `false` writes nothing to stderr; we surface
                // the exit-status fallback message.
                assert!(
                    detail.contains("status") || !detail.is_empty(),
                    "non-empty detail: {detail}"
                );
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn real_scanner_for_spec_timeout_returns_rejected() {
        // Sleep-script against a sub-second timeout — the
        // `recv_timeout` path. Detail mentions the timeout so
        // operators can tune `scanner.timeout_seconds` from log
        // output alone.
        if !Path::new("/bin/sh").exists() {
            return;
        }
        let script = TempDir::new().unwrap();
        let script_path = script.path().join("slow.sh");
        fs::write(&script_path, "#!/bin/sh\nsleep 5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let scanner: Box<dyn Scanner> = Box::new(RealScannerForSpec {
            command: script_path.display().to_string(),
        });
        let outcome = scanner.scan(&script_path, Duration::from_millis(500));
        match outcome {
            ScanOutcome::Rejected { detail } => {
                assert!(
                    detail.contains("timed out"),
                    "timeout reason in detail: {detail}"
                );
            }
            other => panic!("expected Rejected on timeout, got {other:?}"),
        }
    }

    #[test]
    fn truncate_for_reject_caps_long_strings() {
        let long: String = "x".repeat(1000);
        let out = truncate_for_reject(&long);
        assert!(out.chars().count() <= 513, "<= cap + ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_for_reject_passes_short_strings_through() {
        assert_eq!(truncate_for_reject("clean"), "clean");
    }
}
