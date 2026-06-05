//! Terminal styling gate shared across CLI output paths.
//!
//! One decision in one place: whether ANSI styling should be emitted to a
//! given output stream. Honors the `NO_COLOR` convention
//! (<https://no-color.org>) as a global opt-out, then defers to whether the
//! target stream is actually a terminal — so piped or redirected output
//! stays clean. Callers pass the terminal-ness of the stream they write
//! to (`io::stdout().is_terminal()` for stdout paths, `io::stderr()…` for
//! stderr), so a single helper fits both stream kinds.
//!
//! Consolidates the `is_terminal()` gating that was previously open-coded
//! per call site (`bot.rs`, `warn.rs`), now extended with the `NO_COLOR`
//! opt-out. Tracked by the gate sweep (#181); first adopter is
//! `release_notes` (#424).

/// `true` when ANSI styling should be emitted to a stream whose
/// terminal-ness is `stream_is_tty`. `NO_COLOR` (set to any value) forces
/// plain output regardless of the stream.
pub(crate) fn use_color(stream_is_tty: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    stream_is_tty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_color_respects_no_color_then_tty() {
        // `NO_COLOR` is process-global; this is the only reader, so save +
        // restore and exercise both env states within one test to avoid
        // racing a sibling test.
        let saved = std::env::var_os("NO_COLOR");

        // NO_COLOR set → always plain, even on a TTY.
        std::env::set_var("NO_COLOR", "1");
        assert!(!use_color(true), "NO_COLOR set must force plain on a TTY");
        assert!(
            !use_color(false),
            "NO_COLOR set must force plain when piped"
        );

        // NO_COLOR set to empty string still counts (no-color.org: any
        // presence disables).
        std::env::set_var("NO_COLOR", "");
        assert!(!use_color(true), "empty NO_COLOR still disables color");

        // NO_COLOR unset → defer to the stream's terminal-ness.
        std::env::remove_var("NO_COLOR");
        assert!(use_color(true), "TTY with NO_COLOR unset should color");
        assert!(!use_color(false), "piped with NO_COLOR unset stays plain");

        match saved {
            Some(v) => std::env::set_var("NO_COLOR", v),
            None => std::env::remove_var("NO_COLOR"),
        }
    }
}
