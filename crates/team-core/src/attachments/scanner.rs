//! Owned scanner process lifecycle and bounded diagnostics.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{AttachmentScanner, ScanOutcome, Scanner};

/// Production scanner: spawns the operator-configured command with
/// the captured attachment snapshot as a single argument. Polls the owned
/// child until completion or timeout, drains bounded stderr, and kills/reaps
/// the process group so timed-out scanners cannot accumulate.
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

pub(super) struct RealScannerForSpec {
    pub(super) command: String,
}

impl Scanner for RealScannerForSpec {
    fn scan(&self, path: &Path, timeout: Duration) -> ScanOutcome {
        let mut command = Command::new(&self.command);
        command
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                return ScanOutcome::Rejected {
                    detail: format!("scanner spawn failed: {e}"),
                }
            }
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return ScanOutcome::Rejected {
                detail: "scanner stderr unavailable".into(),
            };
        };
        let (tx, rx) = mpsc::channel();
        // Drain the pipe, retaining only a bounded diagnostic prefix. stdout
        // is discarded; no output() call can allocate without a limit.
        thread::spawn(move || {
            let mut stderr = stderr;
            let mut kept = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let keep = n.min(4096usize.saturating_sub(kept.len()));
                kept.extend_from_slice(&buf[..keep]);
            }
            let _ = tx.send(kept);
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(10)),
                Ok(None) => break Err(format!("scanner timed out after {}s", timeout.as_secs())),
                Err(e) => break Err(format!("scanner wait failed: {e}")),
            }
        };
        // Reap the owned child and any descendants retaining the stderr pipe.
        // A scanner's process group is private to this invocation.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        match status {
            Ok(status) if status.success() => ScanOutcome::Clean,
            Ok(status) => {
                let bytes = rx
                    .recv_timeout(Duration::from_millis(250))
                    .unwrap_or_default();
                let detail = String::from_utf8_lossy(&bytes).trim().to_string();
                ScanOutcome::Rejected {
                    detail: if detail.is_empty() {
                        format!("exited with status {status}")
                    } else {
                        truncate_for_reject(&detail)
                    },
                }
            }
            Err(detail) => ScanOutcome::Rejected { detail },
        }
    }
}

/// Cap scanner stderr so a chatty scanner can't blow up the reject
/// notification's wire size or the audit log line. 512 chars is
/// plenty for diagnostic context (the operator's full scanner log
/// is on disk anyway).
pub(super) fn truncate_for_reject(s: &str) -> String {
    const CAP: usize = 512;
    if s.chars().count() <= CAP {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(CAP).collect();
        out.push('…');
        out
    }
}
