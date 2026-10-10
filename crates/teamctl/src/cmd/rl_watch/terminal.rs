use super::*;

pub(super) fn current_winsize() -> PtySize {
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        let fd = libc::STDIN_FILENO;
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            return PtySize {
                rows: ws.ws_row,
                cols: ws.ws_col,
                pixel_width: ws.ws_xpixel,
                pixel_height: ws.ws_ypixel,
            };
        }
    }
    // Last-resort fallback when stdin has no winsize info at all (e.g.
    // running outside any terminal). Not a replay invariant — confirmed
    // there is no recording/replay path that reads back the PTY size.
    PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(unix)]
pub(super) static WINCH_PENDING: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn winch_handler(_: libc::c_int) {
    // async-signal-safe: only touches an AtomicBool.
    WINCH_PENDING.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
pub(super) fn install_winch_handler() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        // Use sa_sigaction as the function pointer slot; it's a union with
        // sa_handler on both Linux and macOS in the libc crate, and we
        // don't need SA_SIGINFO since the handler ignores siginfo_t.
        sa.sa_sigaction = winch_handler as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        // SA_RESTART so blocking reads in the stdin/pty threads aren't
        // killed by EINTR every time the operator resizes.
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
    }
}

/// RAII guard that puts stdin into raw mode on construction and restores
/// the saved termios on drop. `noop()` constructs a guard that does
/// nothing (used when stdin is not a TTY).
pub(super) struct TermiosGuard {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl TermiosGuard {
    #[cfg(unix)]
    pub(super) fn new() -> Self {
        unsafe {
            let fd = libc::STDIN_FILENO;
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return TermiosGuard { saved: None };
            }
            let saved = termios;
            libc::cfmakeraw(&mut termios);
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return TermiosGuard { saved: None };
            }
            TermiosGuard { saved: Some(saved) }
        }
    }

    #[cfg(not(unix))]
    pub(super) fn new() -> Self {
        TermiosGuard {}
    }

    pub(super) fn noop() -> Self {
        #[cfg(unix)]
        {
            TermiosGuard { saved: None }
        }
        #[cfg(not(unix))]
        {
            TermiosGuard {}
        }
    }
}

#[cfg(unix)]
impl Drop for TermiosGuard {
    fn drop(&mut self) {
        if let Some(t) = self.saved {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
}
