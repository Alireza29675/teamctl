//! T-370: keep the host awake (no idle-sleep) while teamctl agents are up,
//! so long-running tasks survive display sleep. macOS-only — `caffeinate -i -s`
//! asserts against idle + system sleep (it does NOT keep the display on, and
//! cannot override lid-closed/clamshell sleep, which is hardware-gated).
//!
//! One host-level process, refcounted: started on `up`, released only when the
//! last teamctl team on the host goes `down`. The refcount source is the
//! host-wide `@teamctl`-tagged tmux listing (see [`super::sessions`]), not the
//! pidfile — so a `down` from any project correctly leaves the keep-awake alive
//! while another team is still up. Non-macOS hosts: every entry point is a no-op.

/// Host-global pidfile. Lives under `$TMPDIR` (per-user on macOS) rather than a
/// project's `.team/state/` because the keep-awake is host-level: project B's
/// `up`/`down` must find the process project A started. Cleared on reboot —
/// which also kills `caffeinate`, so a stale-after-reboot pid self-heals.
#[cfg(any(target_os = "macos", test))]
fn pidfile_path() -> std::path::PathBuf {
    std::env::temp_dir().join("teamctl-caffeinate.pid")
}

/// Parse a pid from pidfile contents. `None` on empty / garbage / non-positive.
#[cfg(any(target_os = "macos", test))]
fn read_pid(contents: &str) -> Option<i32> {
    match contents.trim().parse::<i32>() {
        Ok(pid) if pid > 0 => Some(pid),
        _ => None,
    }
}

/// `true` if `pid` is a live process. Signal 0 probes for existence without
/// delivering a signal; `EPERM` means the process exists but we may not signal
/// it (still alive), `ESRCH` means it is gone.
#[cfg(all(unix, any(target_os = "macos", test)))]
fn pid_alive(pid: i32) -> bool {
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Ensure one host-level `caffeinate -i -s` is running. Idempotent: if the
/// recorded pid is still alive, this is a no-op. Best-effort — a spawn failure
/// warns but never blocks `up` (a host that can idle-sleep is degraded, not
/// fatal).
#[cfg(target_os = "macos")]
pub fn ensure_running() {
    use std::fs;
    use std::process::{Command, Stdio};

    let pf = pidfile_path();
    if let Ok(s) = fs::read_to_string(&pf) {
        if let Some(pid) = read_pid(&s) {
            if pid_alive(pid) {
                return;
            }
        }
    }
    // Detached: stdio nulled so it outlives the fire-and-forget CLI and never
    // writes into a tmux pane. `-i` blocks idle sleep, `-s` blocks system sleep
    // on AC power; neither keeps the display on.
    match Command::new("caffeinate")
        .args(["-i", "-s"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            if let Err(e) = fs::write(&pf, child.id().to_string()) {
                eprintln!("warn · caffeinate: started but could not record pid ({e})");
            }
        }
        Err(e) => {
            eprintln!("warn · caffeinate: could not start, host may idle-sleep ({e})");
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_running() {}

/// Release the keep-awake, but only once no teamctl tmux session remains
/// host-wide (last-one-out). SIGTERMs the recorded pid and clears the pidfile.
/// A stale or already-dead pid is reaped without error.
#[cfg(target_os = "macos")]
pub fn stop_if_last() {
    use std::fs;

    if super::sessions::any_teamctl_session_running() {
        return;
    }
    let pf = pidfile_path();
    if let Ok(s) = fs::read_to_string(&pf) {
        if let Some(pid) = read_pid(&s) {
            if pid_alive(pid) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
        }
    }
    let _ = fs::remove_file(&pf);
}

#[cfg(not(target_os = "macos"))]
pub fn stop_if_last() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pid_parses_valid_and_rejects_garbage() {
        assert_eq!(read_pid("1234\n"), Some(1234));
        assert_eq!(read_pid("  42 "), Some(42));
        assert_eq!(read_pid(""), None);
        assert_eq!(read_pid("abc"), None);
        assert_eq!(read_pid("0"), None);
        assert_eq!(read_pid("-5"), None);
    }

    #[test]
    fn pidfile_path_is_host_global_named() {
        let p = pidfile_path();
        assert_eq!(p.file_name().unwrap(), "teamctl-caffeinate.pid");
        assert_eq!(p.parent().unwrap(), std::env::temp_dir());
    }

    #[cfg(unix)]
    #[test]
    fn pid_alive_true_for_self_false_for_sentinel() {
        assert!(pid_alive(std::process::id() as i32));
        // i32::MAX is far above any OS pid_max → ESRCH → not alive.
        assert!(!pid_alive(i32::MAX));
    }
}
