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
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Lookup contract: forward one tmux key-name to the named session.
/// `key` is already encoded — see `encode_key` for the crossterm →
/// tmux translation. The production implementation (`TmuxKeySender`)
/// blocks on a `tmux send-keys` round-trip; callers on a latency-
/// sensitive thread should wrap it in `AsyncKeySender` so the round-
/// trip happens off the caller's thread (#386).
pub trait KeySender: Send + Sync {
    fn send(&self, session: &str, key: &EncodedKey) -> Result<()>;

    /// Forward one mouse-wheel tick to the named tmux session as a
    /// terminal-history scroll. Implementations target the pane's
    /// copy-mode scroll commands, so the agent's history surfaces the
    /// same way `tmux attach` + wheel does — wheel-up auto-enters
    /// copy-mode, subsequent ticks scroll the buffer. Wheel-down on a
    /// pane not in copy-mode is a no-op (tmux's own behaviour).
    fn scroll(&self, session: &str, direction: ScrollDirection) -> Result<()>;
}

/// Direction of one mouse-wheel tick. Maps to tmux copy-mode commands
/// `scroll-up` / `scroll-down`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
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

/// Production implementation — shells out to `tmux send-keys`, one
/// subprocess per keystroke. The round-trip blocks the caller ~3ms
/// (pure tmux client/server cost; the key delivery itself is free), so
/// the TUI wraps this in `AsyncKeySender` to keep that block off its
/// render/event loop (#386).
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

    fn scroll(&self, session: &str, direction: ScrollDirection) -> Result<()> {
        // ScrollUp first runs `copy-mode -e` so wheel-up on an
        // un-scrolled pane mirrors `tmux attach` + wheel: enter
        // copy-mode and start scrolling. `-e` auto-exits copy-mode
        // once the user scrolls back to the bottom, so wheel-down at
        // the end of history drops the operator cleanly back into
        // the live pane. Already-in-copy-mode is a harmless no-op for
        // the second invocation.
        if matches!(direction, ScrollDirection::Up) {
            let _ = Command::new("tmux")
                .args(["copy-mode", "-e", "-t", session])
                .output()
                .with_context(|| format!("invoke tmux copy-mode -e -t {session}"))?;
        }
        let cmd = match direction {
            ScrollDirection::Up => "scroll-up",
            ScrollDirection::Down => "scroll-down",
        };
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", session, "-X", cmd])
            .output()
            .with_context(|| format!("invoke tmux send-keys -t {session} -X {cmd}"))?;
        Ok(())
    }
}

/// Forward through a shared `KeySender`. Lets a single sender be both
/// moved onto `AsyncKeySender`'s worker thread and retained by the
/// caller (tests inspect the inner mock after the worker drains).
impl<T: KeySender + ?Sized> KeySender for std::sync::Arc<T> {
    fn send(&self, session: &str, key: &EncodedKey) -> Result<()> {
        (**self).send(session, key)
    }

    fn scroll(&self, session: &str, direction: ScrollDirection) -> Result<()> {
        (**self).scroll(session, direction)
    }
}

/// One unit of work for the background sender thread. Mirrors the
/// `KeySender` surface so the worker replays either a keystroke or a
/// scroll tick against the inner (blocking) sender, in arrival order.
enum KeyJob {
    Send {
        session: String,
        key: EncodedKey,
    },
    Scroll {
        session: String,
        direction: ScrollDirection,
    },
}

/// Non-blocking `KeySender` decorator — moves the blocking `tmux`
/// round-trip off the caller's thread. `send`/`scroll` enqueue onto an
/// unbounded channel and return immediately (sub-microsecond); a single
/// background thread drains the channel in FIFO order and replays each
/// job against the wrapped sender.
///
/// Why (#386): the TUI forwards each keystroke inline on its
/// render/event loop (`app::run`). A per-key `tmux send-keys` blocks
/// that loop ~3ms, and a keystroke that lands while the loop is mid
/// `capture-pane` / refresh waits behind it — perceptible input lag.
/// Handing the send to a background thread frees the loop to observe the
/// next key and redraw. The channel is FIFO and single-consumer, so the
/// pane still receives keys in the exact order and form the inline path
/// produced — no batching, no reordering, so the #374 forwarding
/// contract (Esc / Ctrl+C / arrows) is untouched.
///
/// Inner-sender contract: the wrapped `KeySender` must signal failures
/// by returning `Err` (which the worker swallows and moves on, matching
/// the inline path's best-effort `let _ = send(...)`), not by panicking.
/// `TmuxKeySender` honours this — its `tmux` round-trip returns `Err` on
/// any failure and never unwraps — so the worker thread is durable for
/// the life of the app.
pub struct AsyncKeySender {
    tx: Option<Sender<KeyJob>>,
    /// Worker sends `()` here once the queue is drained and closed. Drop
    /// waits on it (bounded) so a normal quit flushes pending keys
    /// without a stuck tmux being able to hang process exit. The `Mutex`
    /// is only to satisfy the `Sync` bound on `KeySender` (`Receiver` is
    /// `Send` but not `Sync`); it's touched solely from `Drop`, never on
    /// the send hot path.
    done: std::sync::Mutex<Receiver<()>>,
}

/// Upper bound on how long `Drop` waits for the worker to flush its
/// queue. Comfortably covers a normal quit (the queue is near-empty —
/// the worker drains each key in ~3ms), but caps the wait so a wedged
/// `tmux` round-trip can't hang the terminal on exit.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

impl AsyncKeySender {
    /// Wrap a blocking sender and spawn its drain thread. `inner` is
    /// moved onto the worker, which runs every job to completion in
    /// arrival order. The thread lives until this `AsyncKeySender` is
    /// dropped.
    pub fn new<K: KeySender + 'static>(inner: K) -> Self {
        let (tx, rx) = mpsc::channel::<KeyJob>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            // `recv` blocks until a job arrives; the loop ends only once
            // the `Sender` is dropped (channel closed) AND the queue is
            // drained, so a buffered burst always flushes before exit.
            while let Ok(job) = rx.recv() {
                match job {
                    KeyJob::Send { session, key } => {
                        let _ = inner.send(&session, &key);
                    }
                    KeyJob::Scroll { session, direction } => {
                        let _ = inner.scroll(&session, direction);
                    }
                }
            }
            // Drained + closed — let `Drop` stop waiting. A dropped
            // receiver (sender already gone) makes this a harmless no-op.
            let _ = done_tx.send(());
        });
        Self {
            tx: Some(tx),
            done: std::sync::Mutex::new(done_rx),
        }
    }
}

impl KeySender for AsyncKeySender {
    fn send(&self, session: &str, key: &EncodedKey) -> Result<()> {
        if let Some(tx) = &self.tx {
            // A closed channel (worker gone) is silent — same best-effort
            // contract as a dropped tmux round-trip in the inline path.
            let _ = tx.send(KeyJob::Send {
                session: session.to_string(),
                key: key.clone(),
            });
        }
        Ok(())
    }

    fn scroll(&self, session: &str, direction: ScrollDirection) -> Result<()> {
        if let Some(tx) = &self.tx {
            let _ = tx.send(KeyJob::Scroll {
                session: session.to_string(),
                direction,
            });
        }
        Ok(())
    }
}

impl Drop for AsyncKeySender {
    fn drop(&mut self) {
        // Close the channel so the worker's `recv` loop ends once the
        // queue drains, then wait (bounded) for it to signal completion,
        // so a fast quit still flushes the last few keys the operator
        // typed. The `DRAIN_TIMEOUT` cap means a wedged `tmux` round-trip
        // can't hang the terminal on exit — on timeout we abandon the
        // (detached) worker and let the OS reap it at process exit.
        self.tx.take();
        if let Ok(done) = self.done.get_mut() {
            let _ = done.recv_timeout(DRAIN_TIMEOUT);
        }
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
    /// Separate `scroll_calls` log for mouse-wheel forwards so
    /// keystroke and scroll surfaces stay independently inspectable.
    #[derive(Default)]
    pub struct MockKeySender {
        pub calls: Mutex<Vec<(String, EncodedKey)>>,
        pub scroll_calls: Mutex<Vec<(String, ScrollDirection)>>,
    }

    impl KeySender for MockKeySender {
        fn send(&self, session: &str, key: &EncodedKey) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((session.to_string(), key.clone()));
            Ok(())
        }

        fn scroll(&self, session: &str, direction: ScrollDirection) -> Result<()> {
            self.scroll_calls
                .lock()
                .unwrap()
                .push((session.to_string(), direction));
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

    #[test]
    fn mock_records_scroll_session_and_direction() {
        use test_support::MockKeySender;
        let mock = MockKeySender::default();
        mock.scroll("t-p-a", ScrollDirection::Up).unwrap();
        mock.scroll("t-p-a", ScrollDirection::Down).unwrap();
        let calls = mock.scroll_calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                ("t-p-a".to_string(), ScrollDirection::Up),
                ("t-p-a".to_string(), ScrollDirection::Down),
            ]
        );
    }

    // `AsyncKeySender` decorator tests. The worker thread drains
    // asynchronously, so the drop-join (`drop(acs)`) is the only
    // synchronisation point that makes the recorded `calls` safe to
    // read — every assertion below runs strictly AFTER the drop.

    #[test]
    fn async_preserves_send_order_and_flushes_on_drop() {
        use test_support::MockKeySender;
        // Shared so the worker owns one clone and the test inspects
        // another after the worker drains.
        let mock = std::sync::Arc::new(MockKeySender::default());
        let session = "t-p-a";
        let submitted = vec![
            EncodedKey {
                args: vec!["-l".into(), "a".into()],
            },
            EncodedKey {
                args: vec!["-l".into(), "b".into()],
            },
            EncodedKey {
                args: vec!["Escape".into()],
            },
            EncodedKey {
                args: vec!["C-c".into()],
            },
        ];

        let acs = AsyncKeySender::new(mock.clone());
        for key in &submitted {
            acs.send(session, key).unwrap();
        }
        // Drop joins the worker, which drains the channel before
        // exiting — the flush-on-quit guarantee. Only now is `calls`
        // race-free to read.
        drop(acs);

        let calls = mock.calls.lock().unwrap();
        let expected: Vec<(String, EncodedKey)> = submitted
            .into_iter()
            .map(|key| (session.to_string(), key))
            .collect();
        assert_eq!(
            *calls, expected,
            "every send must land once, in submission order"
        );
    }

    #[test]
    fn async_drops_no_keys_under_a_burst() {
        use test_support::MockKeySender;
        let mock = std::sync::Arc::new(MockKeySender::default());
        let key = EncodedKey {
            args: vec!["-l".into(), "x".into()],
        };

        let acs = AsyncKeySender::new(mock.clone());
        for _ in 0..100 {
            acs.send("t-p-a", &key).unwrap();
        }
        drop(acs);

        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 100, "all 100 buffered sends must flush");
        assert!(
            calls.iter().all(|(s, k)| s == "t-p-a" && *k == key),
            "no entry may be mangled or mis-routed"
        );
    }

    #[test]
    fn async_routes_scroll_through_the_same_channel_in_order() {
        use test_support::MockKeySender;
        let mock = std::sync::Arc::new(MockKeySender::default());
        let session = "t-p-a";
        let key_a = EncodedKey {
            args: vec!["-l".into(), "a".into()],
        };
        let key_b = EncodedKey {
            args: vec!["-l".into(), "b".into()],
        };

        let acs = AsyncKeySender::new(mock.clone());
        // Interleave sends and scrolls — both ride the one FIFO queue.
        acs.send(session, &key_a).unwrap();
        acs.scroll(session, ScrollDirection::Up).unwrap();
        acs.send(session, &key_b).unwrap();
        acs.scroll(session, ScrollDirection::Down).unwrap();
        drop(acs);

        let calls = mock.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![(session.to_string(), key_a), (session.to_string(), key_b),],
            "sends preserve relative order across interleaved scrolls"
        );
        let scrolls = mock.scroll_calls.lock().unwrap();
        assert_eq!(
            *scrolls,
            vec![
                (session.to_string(), ScrollDirection::Up),
                (session.to_string(), ScrollDirection::Down),
            ],
            "scrolls preserve relative order across interleaved sends"
        );
    }

    #[test]
    fn async_round_trips_args_byte_identically() {
        use test_support::MockKeySender;
        // The channel must move the already-encoded `args` through
        // untouched. A mangled round-trip would silently break the
        // `\;` escape (#114) or the named-key forwarding contract
        // (#374), so pin both arg vectors exactly.
        let mock = std::sync::Arc::new(MockKeySender::default());
        let session = "t-p-a";
        let escaped_semicolon = EncodedKey {
            args: vec!["-l".into(), "\\;".into()],
        };
        let named_escape = EncodedKey {
            args: vec!["Escape".into()],
        };

        let acs = AsyncKeySender::new(mock.clone());
        acs.send(session, &escaped_semicolon).unwrap();
        acs.send(session, &named_escape).unwrap();
        drop(acs);

        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].1.args,
            vec!["-l".to_string(), "\\;".to_string()],
            "the `\\;` escape must survive the channel round-trip intact"
        );
        assert_eq!(
            calls[1].1.args,
            vec!["Escape".to_string()],
            "a named key must survive the channel round-trip intact"
        );
    }
}
