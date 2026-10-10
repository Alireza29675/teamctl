use super::scanner::{truncate_for_reject, RealScannerForSpec};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

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
    // `/bin/sleep 5` against a sub-second timeout — the
    // `recv_timeout` path. Detail mentions the timeout so
    // operators can tune `scanner.timeout_seconds` from log
    // output alone.
    //
    // The previous shape wrote a `#!/bin/sh\nsleep 5` script
    // into a TempDir, chmod'd it +x, and pointed the scanner
    // at it. That hit a flake on otis's #151 CI run: the
    // chmod/exec-bit visibility race on a forked subprocess
    // surfaced as a "scanner spawn failed: permission denied"
    // result instead of a timeout. Using `/bin/sleep` directly
    // — present on every linux runner — eliminates the FS
    // race entirely. The `path` arg passed to scan() is
    // ceremonial here (the trait forwards it as a process
    // arg; sleep treats it as a duration, which is exactly
    // what we want).
    if !Path::new("/bin/sleep").exists() {
        return;
    }
    let scanner: Box<dyn Scanner> = Box::new(RealScannerForSpec {
        command: "/bin/sleep".into(),
    });
    let outcome = scanner.scan(Path::new("5"), Duration::from_millis(500));
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

#[test]
fn bounded_read_checks_actual_bytes_not_only_metadata() {
    let err = read_bounded(&b"0123456789"[..], 8).unwrap_err();
    assert_eq!(err, RejectReason::TooLarge { size: 9, cap: 8 });
    assert_eq!(read_bounded(&b"01234567"[..], 8).unwrap(), b"01234567");
}

#[test]
fn directories_are_not_attachments() {
    let dir = TempDir::new().unwrap();
    let cfg = cfg_with_root(dir.path(), 1024);
    assert!(matches!(
        check_and_read(&cfg, dir.path(), None),
        Err(RejectReason::ReadFailed(_))
    ));
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_waiting_for_a_writer() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fifo");
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let before = Instant::now();
    let cfg = cfg_with_root(dir.path(), 1024);
    assert!(matches!(
        check_and_read(&cfg, &path, None),
        Err(RejectReason::ReadFailed(_))
    ));
    assert!(before.elapsed() < Duration::from_secs(1));
}

struct ChangingScanner {
    source: PathBuf,
    modify_snapshot: bool,
}
impl Scanner for ChangingScanner {
    fn scan(&self, path: &Path, _timeout: Duration) -> ScanOutcome {
        assert_eq!(fs::read(path).unwrap(), b"safe");
        assert_ne!(path, self.source);
        if self.modify_snapshot {
            fs::write(path, b"evil").unwrap();
        } else {
            fs::write(&self.source, b"source replaced after capture").unwrap();
        }
        ScanOutcome::Clean
    }
}

#[test]
fn scanner_certifies_captured_bytes_even_when_source_changes() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("input.txt");
    fs::write(&source, b"safe").unwrap();
    let mut cfg = cfg_with_root(dir.path(), 1024);
    cfg.scanner = Some(AttachmentScanner {
        command: "unused".into(),
        timeout_seconds: 1,
    });
    let scanner = ChangingScanner {
        source: source.clone(),
        modify_snapshot: false,
    };
    let accepted = check_and_read_with_metadata(&cfg, &source, Some(&scanner)).unwrap();
    assert_eq!(accepted.bytes, b"safe");
    assert_eq!(accepted.resolved, source.canonicalize().unwrap());
    assert_eq!(accepted.blake3_hex, blake3::hash(b"safe").to_hex().as_str());
}

#[test]
fn scanner_cannot_replace_the_bytes_it_certifies() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("input.txt");
    fs::write(&source, b"safe").unwrap();
    let mut cfg = cfg_with_root(dir.path(), 1024);
    cfg.scanner = Some(AttachmentScanner {
        command: "unused".into(),
        timeout_seconds: 1,
    });
    let scanner = ChangingScanner {
        source: source.clone(),
        modify_snapshot: true,
    };
    assert!(matches!(
        check_and_read(&cfg, &source, Some(&scanner)),
        Err(RejectReason::ScannerRejected { .. })
    ));
}

#[test]
fn staging_repairs_equal_size_corrupt_cache() {
    let dir = TempDir::new().unwrap();
    let accepted = accepted_for(b"safe");
    let path = stage_to_tempfile(dir.path(), &accepted).unwrap();
    fs::write(&path, b"evil").unwrap();
    assert_eq!(stage_to_tempfile(dir.path(), &accepted).unwrap(), path);
    assert_eq!(fs::read(&path).unwrap(), b"safe");
}

#[test]
fn concurrent_identical_staging_is_atomic_and_leaves_no_temporary_files() {
    let dir = TempDir::new().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let barrier = barrier.clone();
            let staging = dir.path().to_path_buf();
            thread::spawn(move || {
                let accepted = accepted_for(&vec![b'x'; 64 * 1024]);
                barrier.wait();
                let path = stage_to_tempfile(&staging, &accepted).unwrap();
                assert_eq!(fs::read(path).unwrap(), accepted.bytes);
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn staging_replaces_symlink_without_writing_its_target() {
    let dir = TempDir::new().unwrap();
    let accepted = accepted_for(b"safe");
    let outside = dir.path().join("outside");
    fs::write(&outside, b"evil").unwrap();
    let staging = dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    let link = staging.join(&accepted.blake3_hex);
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert_eq!(stage_to_tempfile(&staging, &accepted).unwrap(), link);
    assert_eq!(fs::read(&outside).unwrap(), b"evil");
    assert!(!fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn timed_out_scanner_is_killed_before_it_can_mutate_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("scanner");
    let marker = dir.path().join("should-not-exist");
    fs::write(&script, "#!/bin/sh\nsleep 1\nprintf leaked > \"$1\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let scanner = RealScannerForSpec {
        command: script.to_string_lossy().into(),
    };
    assert!(matches!(
        scanner.scan(&marker, Duration::from_millis(100)),
        ScanOutcome::Rejected { .. }
    ));
    thread::sleep(Duration::from_millis(1100));
    assert!(
        !marker.exists(),
        "owned child and sleep descendant must be terminated"
    );
}

#[cfg(unix)]
#[test]
fn scanner_diagnostics_are_bounded_while_pipe_is_drained() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("scanner");
    fs::write(&script, "#!/bin/sh\ni=0\nwhile [ $i -lt 2048 ]; do printf '%01024d' 0 >&2; i=$((i+1)); done\nexit 1\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let scanner = RealScannerForSpec {
        command: script.to_string_lossy().into(),
    };
    match scanner.scan(Path::new("unused"), Duration::from_secs(5)) {
        ScanOutcome::Rejected { detail } => assert_eq!(detail.chars().count(), 513),
        ScanOutcome::Clean => panic!("exit 1 cannot pass scanner"),
    }
}

#[cfg(unix)]
#[test]
fn scanner_replacing_snapshot_with_fifo_cannot_block_verification() {
    struct FifoScanner;
    impl Scanner for FifoScanner {
        fn scan(&self, path: &Path, _timeout: Duration) -> ScanOutcome {
            use std::os::unix::ffi::OsStrExt;
            fs::remove_file(path).unwrap();
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            ScanOutcome::Clean
        }
    }
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"safe").unwrap();
    let mut cfg = cfg_with_root(dir.path(), 1024);
    cfg.scanner = Some(AttachmentScanner {
        command: "unused".into(),
        timeout_seconds: 1,
    });
    let start = Instant::now();
    assert!(matches!(
        check_and_read(&cfg, &source, Some(&FifoScanner)),
        Err(RejectReason::ReadFailed(_))
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn descriptor_confinement_uses_the_opened_file_after_parent_replacement() {
    let dir = TempDir::new().unwrap();
    let allowed = dir.path().join("allowed");
    let outside = dir.path().join("outside");
    fs::create_dir(&allowed).unwrap();
    fs::create_dir(&outside).unwrap();
    let input = outside.join("input");
    fs::write(&input, b"outside bytes").unwrap();
    let handle = open_regular(&input).unwrap();
    // A pathname can be made to name an allowed file after an outside open.
    // Kernel descriptor identity must still point at the outside file.
    let fake_allowed_path = allowed.join("input");
    fs::write(&fake_allowed_path, b"safe").unwrap();
    let actual = descriptor_path(&handle, &fake_allowed_path).unwrap();
    assert_eq!(actual, input.canonicalize().unwrap());
    assert!(!is_within_any_root(
        &actual,
        &[allowed.canonicalize().unwrap()]
    ));
}

#[cfg(unix)]
#[test]
fn staging_replaces_fifo_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    let dir = TempDir::new().unwrap();
    let accepted = accepted_for(b"safe");
    let path = dir.path().join(&accepted.blake3_hex);
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    stage_to_tempfile(dir.path(), &accepted).unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(fs::read(&path).unwrap(), b"safe");
}
