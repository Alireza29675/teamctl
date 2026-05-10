//! Key forwarding — abstracts how the UI streams keystrokes into a
//! tmux pane so tests can stub it out. Production hits
//! `tmux send-keys`; tests pass a `MockKeySender` recording every
//! call. Mirrors the trait + prod + mock shape `pane.rs` uses for
//! capture so the two surfaces evolve together.
//!
//! Used by `Stage::StreamKeys` (the ticket-#108 modal): once stream
//! mode is active, every operator keystroke that isn't `Esc` gets
//! translated to a tmux key-name and shipped over.

use std::process::Command;

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Lookup contract: forward one tmux key-name to the named session.
/// `key` is already encoded — see `encode_key` for the crossterm →
/// tmux translation. Implementations must treat the call as
/// fire-and-forget at the operator's typing rate; a per-call
/// `tmux send-keys` round-trip is acceptable for v1 (the 50ms event
/// poll already gates throughput).
pub trait KeySender: Send + Sync {
    fn send(&self, session: &str, key: &EncodedKey) -> Result<()>;
}

/// One encoded keystroke ready for `tmux send-keys`. Carries the
/// argument list so the prod impl can shell out without re-doing the
/// translation, and so tests can inspect exactly what the encoder
/// produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedKey {
    /// Args appended after `tmux send-keys -t <session>`. Either a
    /// single key-name (`"C-c"`, `"Enter"`) or, for printable chars,
    /// `["-l", "<char>"]` so tmux treats the byte literally and
    /// doesn't try to parse it as a key-name.
    pub args: Vec<String>,
}

impl EncodedKey {
    fn named(name: impl Into<String>) -> Self {
        Self {
            args: vec![name.into()],
        }
    }

    fn literal(text: impl Into<String>) -> Self {
        Self {
            args: vec!["-l".into(), text.into()],
        }
    }
}

/// Translate a crossterm `KeyEvent` to the form `tmux send-keys`
/// expects. Returns `None` for keys we deliberately drop (release
/// events on kitty-protocol terminals, modifier-only presses).
///
/// Convention:
/// - Plain printable chars → `-l <char>` (literal, sidesteps tmux's
///   key-name parsing on tokens like `;` or `~`).
/// - Modifier combos and named keys → tmux key-name form
///   (`C-c`, `M-x`, `S-Tab`, `Enter`, `BSpace`, `Up`, `F4`, …).
/// - Shift on a printable char is already reflected in the char
///   itself (crossterm gives `Char('A')` for shift+a), so `S-` is
///   only emitted for named keys (`S-Tab`, `S-Up`).
pub fn encode_key(ev: KeyEvent) -> Option<EncodedKey> {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);

    // Modifier prefix shared by named-key and ctrl/alt-char paths.
    let prefix = match (ctrl, alt) {
        (true, true) => "C-M-",
        (true, false) => "C-",
        (false, true) => "M-",
        (false, false) => "",
    };

    match ev.code {
        // Printable chars. Ctrl+letter / Alt+letter take the named
        // form (`C-c`); everything else goes through `-l` literal so
        // tmux doesn't reinterpret tokens like `~`, `:`.
        KeyCode::Char(c) => {
            if ctrl || alt {
                // tmux wants Ctrl+letter chords lowercased: `C-c`,
                // not `C-C`. Same convention for Alt.
                let normalised = c.to_ascii_lowercase();
                Some(EncodedKey::named(format!("{prefix}{normalised}")))
            } else if c == ';' {
                // tmux's command-list parser treats a bare `;` arg
                // as the command separator, not as data — even
                // under `-l` the arg is tokenised first, so a
                // literal-mode `;` keystroke is silently dropped
                // before it ever reaches the pane. Escape it as
                // `\;` so the parser passes `;` through to the
                // `-l` handler as data. (Reported by qa on PR
                // #114 with a live repro: typing "off); buy" lost
                // the `;`.)
                Some(EncodedKey::literal("\\;".to_string()))
            } else {
                Some(EncodedKey::literal(c.to_string()))
            }
        }
        // Named keys — modifier-prefixed form.
        KeyCode::Enter => Some(EncodedKey::named(format!("{prefix}Enter"))),
        KeyCode::Tab => {
            // Shift+Tab uses tmux's BTab name. Otherwise the prefix
            // covers C-/M- combos.
            if shift && !ctrl && !alt {
                Some(EncodedKey::named("BTab"))
            } else {
                Some(EncodedKey::named(format!("{prefix}Tab")))
            }
        }
        KeyCode::BackTab => Some(EncodedKey::named("BTab")),
        KeyCode::Backspace => Some(EncodedKey::named(format!("{prefix}BSpace"))),
        KeyCode::Delete => Some(EncodedKey::named(format!("{prefix}DC"))),
        KeyCode::Up => Some(EncodedKey::named(format!("{prefix}Up"))),
        KeyCode::Down => Some(EncodedKey::named(format!("{prefix}Down"))),
        KeyCode::Left => Some(EncodedKey::named(format!("{prefix}Left"))),
        KeyCode::Right => Some(EncodedKey::named(format!("{prefix}Right"))),
        KeyCode::Home => Some(EncodedKey::named(format!("{prefix}Home"))),
        KeyCode::End => Some(EncodedKey::named(format!("{prefix}End"))),
        KeyCode::PageUp => Some(EncodedKey::named(format!("{prefix}PPage"))),
        KeyCode::PageDown => Some(EncodedKey::named(format!("{prefix}NPage"))),
        KeyCode::Insert => Some(EncodedKey::named(format!("{prefix}IC"))),
        KeyCode::F(n) if (1..=12).contains(&n) => Some(EncodedKey::named(format!("{prefix}F{n}"))),
        // Esc is the stream-mode exit chord — handled at the dispatch
        // layer before encoding, so reaching this arm means the
        // operator fired a literal Esc inside some other path.
        // Forward it as `Escape` for completeness.
        KeyCode::Esc => Some(EncodedKey::named("Escape")),
        // Modifier-only presses, media keys, kitty-protocol release
        // events — drop silently.
        _ => None,
    }
}

/// Production implementation — shells out to `tmux send-keys`. Per-
/// keystroke; v1 doesn't batch (the 50ms event poll already gates
/// throughput, and per-call latency stays below typing speed).
#[derive(Debug, Default, Clone, Copy)]
pub struct TmuxKeySender;

impl KeySender for TmuxKeySender {
    fn send(&self, session: &str, key: &EncodedKey) -> Result<()> {
        let mut cmd = Command::new("tmux");
        cmd.args(["send-keys", "-t", session]);
        for arg in &key.args {
            cmd.arg(arg);
        }
        let output = cmd
            .output()
            .with_context(|| format!("invoke tmux send-keys -t {session}"))?;
        // Non-zero exit (e.g. session vanished mid-stream) is logged
        // by the absence of expected output in the next refresh
        // tick; we don't want one bad frame to kill stream-mode.
        let _ = output;
        Ok(())
    }
}

/// Test fixtures. Made `pub` (rather than `#[cfg(test)]`) so the
/// integration tests in `tests/` can reach them — same pattern as
/// `compose::test_support` and `mailbox::test_support`.
pub mod test_support {
    use super::*;
    use std::sync::Mutex;

    /// Recording stub. Captures every `(session, encoded)` pair so
    /// tests can assert which session was targeted with which key.
    #[derive(Default)]
    pub struct MockKeySender {
        pub calls: Mutex<Vec<(String, EncodedKey)>>,
    }

    impl KeySender for MockKeySender {
        fn send(&self, session: &str, key: &EncodedKey) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((session.to_string(), key.clone()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn printable_char_uses_literal_form() {
        let enc = encode_key(k(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
        assert_eq!(enc.args, vec!["-l".to_string(), "a".to_string()]);
    }

    #[test]
    fn shifted_printable_char_keeps_literal_form() {
        // crossterm pre-shifts the char; the encoder doesn't double
        // up by also emitting `S-` for printables.
        let enc = encode_key(k(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(enc.args, vec!["-l".to_string(), "A".to_string()]);
    }

    #[test]
    fn punctuation_uses_literal_form() {
        // `~` is the canonical example: tmux would read it as a
        // key-name (`~`), literal mode forwards it as the typed
        // character. Doesn't trigger the `;` escape path.
        let enc = encode_key(k(KeyCode::Char('~'), KeyModifiers::NONE)).unwrap();
        assert_eq!(enc.args, vec!["-l".to_string(), "~".to_string()]);
    }

    #[test]
    fn semicolon_is_backslash_escaped_in_literal_form() {
        // qa-found regression on PR #114: a bare `;` arg is consumed
        // by tmux's own command-list parser as the command separator
        // and never reaches the pane. Escaping it as `\;` survives
        // the parse and lands in the pane as `;`. Pin both the
        // exact arg shape and the path-of-arrival so a future
        // refactor can't quietly drop the escape.
        let enc = encode_key(k(KeyCode::Char(';'), KeyModifiers::NONE)).unwrap();
        assert_eq!(
            enc.args,
            vec!["-l".to_string(), "\\;".to_string()],
            "bare `;` must be sent as `\\;` so tmux's command parser \
             doesn't eat it as a separator"
        );
    }

    #[test]
    fn ctrl_c_passes_through_as_named_chord() {
        // Issue #108 explicitly requires Ctrl+C to forward to the
        // agent (SIGINT), not be intercepted as a stream-mode exit.
        let enc = encode_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(enc.args, vec!["C-c".to_string()]);
    }

    #[test]
    fn ctrl_uppercase_normalises_to_lowercase() {
        // Some terminals emit Ctrl+Shift+C as `Char('C')` + CONTROL;
        // tmux wants `C-c`, not `C-C`.
        let enc = encode_key(k(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ))
        .unwrap();
        assert_eq!(enc.args, vec!["C-c".to_string()]);
    }

    #[test]
    fn alt_char_uses_named_form() {
        let enc = encode_key(k(KeyCode::Char('x'), KeyModifiers::ALT)).unwrap();
        assert_eq!(enc.args, vec!["M-x".to_string()]);
    }

    #[test]
    fn ctrl_alt_char_combines_prefixes() {
        let enc = encode_key(k(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ))
        .unwrap();
        assert_eq!(enc.args, vec!["C-M-a".to_string()]);
    }

    #[test]
    fn enter_named() {
        let enc = encode_key(k(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert_eq!(enc.args, vec!["Enter".to_string()]);
    }

    #[test]
    fn backspace_named() {
        let enc = encode_key(k(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        assert_eq!(enc.args, vec!["BSpace".to_string()]);
    }

    #[test]
    fn arrows_named() {
        for (code, name) in [
            (KeyCode::Up, "Up"),
            (KeyCode::Down, "Down"),
            (KeyCode::Left, "Left"),
            (KeyCode::Right, "Right"),
        ] {
            let enc = encode_key(k(code, KeyModifiers::NONE)).unwrap();
            assert_eq!(enc.args, vec![name.to_string()], "encoding {code:?}");
        }
    }

    #[test]
    fn shift_tab_uses_btab() {
        // tmux's name for Shift+Tab is `BTab`; it doesn't accept
        // `S-Tab`. crossterm may deliver this as either Tab+SHIFT
        // or BackTab — both routes need to reach `BTab`.
        let from_tab = encode_key(k(KeyCode::Tab, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(from_tab.args, vec!["BTab".to_string()]);
        let from_backtab = encode_key(k(KeyCode::BackTab, KeyModifiers::NONE)).unwrap();
        assert_eq!(from_backtab.args, vec!["BTab".to_string()]);
    }

    #[test]
    fn function_keys_named() {
        let enc = encode_key(k(KeyCode::F(7), KeyModifiers::NONE)).unwrap();
        assert_eq!(enc.args, vec!["F7".to_string()]);
        let ctrl_f4 = encode_key(k(KeyCode::F(4), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(ctrl_f4.args, vec!["C-F4".to_string()]);
    }

    #[test]
    fn page_keys_use_tmux_short_names() {
        // tmux uses `PPage`/`NPage` for PageUp/PageDown.
        assert_eq!(
            encode_key(k(KeyCode::PageUp, KeyModifiers::NONE))
                .unwrap()
                .args,
            vec!["PPage".to_string()]
        );
        assert_eq!(
            encode_key(k(KeyCode::PageDown, KeyModifiers::NONE))
                .unwrap()
                .args,
            vec!["NPage".to_string()]
        );
    }

    #[test]
    fn mock_records_session_and_key() {
        use test_support::MockKeySender;
        let mock = MockKeySender::default();
        let enc = encode_key(k(KeyCode::Char('h'), KeyModifiers::NONE)).unwrap();
        mock.send("t-p-a", &enc).unwrap();
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "t-p-a");
        assert_eq!(calls[0].1, enc);
    }
}
