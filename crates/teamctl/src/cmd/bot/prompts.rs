use super::*;

/// Wrap `s` in an ANSI SGR code when color is warranted for stdout; plain
/// text otherwise (piped/redirected, or `NO_COLOR` set, stays clean).
/// Gates on `crate::term::use_color(...)` — the same NO_COLOR + `is_terminal()`
/// check warn.rs adopted (T-181), so `NO_COLOR=1` suppresses the wizard's
/// color even on a TTY (here stdout, where the wizard prints).
pub(super) fn styled(code: &str, s: &str) -> String {
    style_with(code, s, crate::term::use_color(io::stdout().is_terminal()))
}

/// Color-decision-free core of [`styled`], split out so the gate is
/// unit-testable without a real terminal. (T-181)
pub(super) fn style_with(code: &str, s: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub(super) fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read stdin")?;
    Ok(line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string())
}

/// Read one line and strip the trailing newline (and a preceding CR),
/// matching [`prompt`]'s capture behaviour exactly so swapping a field
/// from `prompt` to `prompt_secret` changes only the echo, never the
/// captured value. Split out so the value-correctness contract is
/// unit-testable without a terminal.
pub(super) fn capture_line<R: BufRead>(mut reader: R) -> io::Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string())
}

/// Prompt for a secret (the Telegram bot token). On an interactive
/// unix terminal the input is masked: each typed *or* pasted character
/// echoes a single `*`, so the real token never appears in the
/// terminal, scrollback, a screen-share, or a recording (T-314). The
/// captured value is unaffected — masking is a display concern only.
/// Backspace clears the whole entry (the token is pasted as a blob;
/// re-pasting beats editing a hidden secret).
///
/// Non-interactive stdin (pipe/redirect — tests, automation) has no
/// terminal to mask and `tcgetattr` would fail on a non-tty, so it
/// falls back to a plain read. Non-unix also falls back; the CI matrix
/// and supported install targets are POSIX, where the masking is
/// effective.
pub(super) fn prompt_secret(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().ok();

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let fd = io::stdin().as_raw_fd();
        // SAFETY: `isatty` on any fd is defined and side-effect-free.
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        if is_tty {
            // Restore the original terminal attributes on every exit
            // path (normal return, `?` early-return, panic) so a
            // failure can't strand the terminal with echo disabled.
            struct RestoreEcho {
                fd: i32,
                original: libc::termios,
            }
            impl Drop for RestoreEcho {
                fn drop(&mut self) {
                    // SAFETY: `original` was filled by a successful
                    // `tcgetattr` on this same fd.
                    unsafe {
                        libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
                    }
                }
            }

            // SAFETY: `termios` is a C POD; `tcgetattr` fully
            // initialises it for a valid terminal fd.
            let mut term: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut term) } != 0 {
                return Err(io::Error::last_os_error()).context("tcgetattr (mask token input)");
            }
            let _restore = RestoreEcho { fd, original: term };
            // Clear ECHO (we draw our own `*`) and ICANON (read
            // byte-by-byte instead of line-buffered) while keeping ISIG
            // so Ctrl-C still interrupts. The drop-guard restores every
            // original flag on exit.
            term.c_lflag &= !(libc::ECHO | libc::ICANON);
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) } != 0 {
                return Err(io::Error::last_os_error()).context("tcsetattr mask token input");
            }

            // Read raw input bytes until Enter/EOF, echoing one `*` per
            // printable byte. Backspace clears the whole entry and wipes
            // the drawn asterisks. The raw stream is buffered verbatim and
            // turned into the final value by `process_secret_bytes`, so a
            // pasted token survives byte-for-byte regardless of the
            // on-screen masking — exactly what `capture_line` returned.
            use io::Read;
            let mut stdin = io::stdin().lock();
            let mut stdout = io::stdout();
            let mut raw: Vec<u8> = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                if stdin.read(&mut byte).context("read stdin")? == 0 {
                    break; // EOF — treat like Enter.
                }
                match byte[0] {
                    // Enter (CR or LF): done.
                    b'\r' | b'\n' => break,
                    // Backspace / Delete: clear the whole entry and erase
                    // the asterisks. `\x1b[J` clears from the cursor to the
                    // end of the screen, so the wipe stays correct even when
                    // a long or pasted token wrapped onto extra rows.
                    0x7f | 0x08 => {
                        raw.push(byte[0]);
                        write!(stdout, "\r{msg}\x1b[J").ok();
                        stdout.flush().ok();
                    }
                    // Printable: keep the byte, show a `*`.
                    b if b >= 0x20 && b != 0x7f => {
                        raw.push(b);
                        write!(stdout, "*").ok();
                        stdout.flush().ok();
                    }
                    // Other control bytes: buffered into `raw` (draw
                    // nothing); `process_secret_bytes` drops them from the
                    // final value.
                    _ => raw.push(byte[0]),
                }
            }
            // The user's Enter wasn't echoed — advance the line so the
            // next output doesn't run onto the prompt text.
            println!();
            return process_secret_bytes(&raw);
        }
    }

    capture_line(io::stdin().lock()).context("read stdin")
}

/// Decode the raw bytes captured by the masked secret reader into the
/// final value, independent of the on-screen masking. Mirrors the
/// reader's per-byte rules: printable bytes (`>= 0x20`, not DEL) are kept
/// verbatim; Backspace/Delete (`0x7f`/`0x08`) clears the whole buffer
/// (re-pasting beats editing a hidden secret); CR/LF ends input; any
/// other control byte is dropped. The survivors are decoded once as UTF-8
/// so a pasted multibyte token round-trips exactly.
pub(super) fn process_secret_bytes(bytes: &[u8]) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    for &b in bytes {
        match b {
            b'\r' | b'\n' => break,
            0x7f | 0x08 => buf.clear(),
            b if b >= 0x20 && b != 0x7f => buf.push(b),
            _ => {}
        }
    }
    String::from_utf8(buf).map_err(|e| anyhow!("token contains invalid UTF-8: {e}"))
}

pub(super) fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    let raw = prompt(&format!("{label} [{default}]: "))?;
    let raw = raw.trim();
    Ok(if raw.is_empty() {
        default.to_string()
    } else {
        raw.to_string()
    })
}

pub(super) fn confirm(msg: &str, default_yes: bool) -> Result<bool> {
    let raw = prompt(msg)?.trim().to_lowercase();
    if raw.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(raw.as_str(), "y" | "yes"))
}

// ── .env file write ────────────────────────────────────────────────
