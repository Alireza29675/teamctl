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
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::compose::Attachments;

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
}
