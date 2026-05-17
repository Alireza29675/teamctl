//! Detail-pane → inner-tmux-session size sync.
//!
//! Background: teamctl-ui's `Detail` pane reads the focused agent's
//! tmux scrollback via `tmux capture-pane` (see `pane::TmuxPaneSource`)
//! and renders the captured content inside a ratatui rect. The inner
//! tmux session is spawned with `-x 200 -y 50` (see
//! `team-core::supervisor`) and stays that size until something tells
//! it to resize. T-098 / #99 wired SIGWINCH propagation through
//! `rl-watch` so the inner session reflows when the *outer* host
//! terminal resizes — but that signal only fires on real OS terminal
//! changes, not on teamctl-ui's own layout shifts.
//!
//! Net effect (T-199): when the operator resizes the host terminal so
//! that teamctl-ui's `Detail` rect becomes smaller than 200×50, claude
//! keeps rendering at the inner pane's original 200×50, the captured
//! output is wider+taller than the rect, and the operator sees an
//! overflowing pane.
//!
//! Fix: after every `terminal.draw`, compute the `Detail` rect the
//! Triptych layout would produce for the current terminal size and
//! call `tmux resize-window -t <session> -x W -y H` to keep the inner
//! session sized to match. It must be `resize-window`, **not**
//! `resize-pane`: the agent session is detached, single-pane, and
//! clientless, and `resize-pane` cannot shrink the sole pane of a
//! clientless window — that wrong verb is the #312 recurrence (see
//! [`TmuxPaneResizer`]). Cache the last value we pushed per session
//! so the common case (no resize, no focus switch) is a HashMap
//! lookup, not a subprocess spawn.

use std::collections::HashMap;
use std::process::Command;

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Compute the `Rect` the Triptych layout would allocate to the
/// `Detail` pane given a total terminal area and whether the approvals
/// stripe is visible. Mirrors `triptych::Triptych::render` so the
/// run-loop sync path and the actual render path stay in lockstep —
/// any future tweak to Triptych geometry must update this helper too.
///
/// Returns `None` when the area is too small for the layout to produce
/// a non-empty Detail rect (e.g. an 80×24 terminal in the middle of a
/// resize-down before crossterm catches up). The caller skips the
/// sync in that case rather than push a degenerate size to tmux.
pub fn triptych_detail_area(total: Rect, has_pending_approvals: bool) -> Option<Rect> {
    if total.width == 0 || total.height == 0 {
        return None;
    }
    let body = if has_pending_approvals {
        // One-line approvals stripe at the top.
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(total);
        v[1]
    } else {
        total
    };
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28), // agents sidebar
            Constraint::Min(0),     // right-stack
        ])
        .split(body);
    let right_stack = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(3, 5), Constraint::Ratio(2, 5)])
        .split(outer[1]);
    let detail = right_stack[0];
    if detail.width == 0 || detail.height == 0 {
        return None;
    }
    Some(detail)
}

/// Decide whether to push a `tmux resize-window` to `session` given
/// the current Detail dimensions and the last size we already pushed
/// for this session. The common case (same agent focused, no resize)
/// is a no-op; we only fire the subprocess when the size has actually
/// changed or we haven't synced this session before.
pub fn should_sync(
    cache: &HashMap<String, (u16, u16)>,
    session: &str,
    current: (u16, u16),
) -> bool {
    cache.get(session) != Some(&current)
}

/// Pushes a `tmux resize-window` for a session. Production resizers
/// shell out via [`TmuxPaneResizer`]; tests pass a stub that records
/// calls without touching tmux.
pub trait PaneResizer: Send + Sync {
    /// Best-effort resize. Implementations should not panic or
    /// propagate errors — a missing/killed session is a normal
    /// transient state, not a fatal condition. The cache should
    /// advance regardless so we don't retry-spam a dead session.
    fn resize(&self, session: &str, cols: u16, rows: u16);
}

/// `tmux` argv that resizes `session`'s window to `cols`×`rows`.
///
/// Pulled out as a pure function so the exact subcommand is
/// unit-pinned. It MUST be `resize-window`, never `resize-pane` — see
/// the anti-regression note on [`TmuxPaneResizer`].
fn resize_window_argv(session: &str, cols: u16, rows: u16) -> [String; 7] {
    [
        "resize-window".to_string(),
        "-t".to_string(),
        session.to_string(),
        "-x".to_string(),
        cols.to_string(),
        "-y".to_string(),
        rows.to_string(),
    ]
}

/// Production implementation — shells out to **`tmux resize-window`**.
///
/// It MUST be `resize-window`, **not** `resize-pane`. The agent
/// session is a detached, single-pane, **clientless** tmux session
/// created at `-x 200 -y 50` (`team-core::supervisor`). `resize-pane`
/// only redistributes space *within* a window's existing geometry —
/// for the sole pane of a clientless window it is a silent no-op, so
/// the captured content stays 200×50 and overflows the smaller Detail
/// rect. Only `resize-window` changes the window (and hence its sole
/// pane) for a session with no attached client. This exact "right
/// trigger, wrong verb" error is why the bug recurred across
/// #99 → T-199/#210 → #312. Do NOT "simplify" this back to
/// `resize-pane`. (`resize-window` is tmux ≥ 2.9, 2018 — well below
/// any tmux the supervisor can drive.)
///
/// `-t <session>` targets the agent's session; `-x W -y H` set the
/// window size. Stdout/stderr are dropped: a failure (session gone,
/// tmux not on PATH) is silently ignored and the cache still advances
/// so a fresh spawn next tick can re-sync.
#[derive(Debug, Default, Clone, Copy)]
pub struct TmuxPaneResizer;

impl PaneResizer for TmuxPaneResizer {
    fn resize(&self, session: &str, cols: u16, rows: u16) {
        let _ = Command::new("tmux")
            .args(resize_window_argv(session, cols, rows))
            .status();
    }
}

/// Shared test fakes — mirrors `compose::test_support` /
/// `mailbox::test_support` / `keysender::test_support`. The
/// in-memory `MockPaneResizer` records every call so unit tests in
/// other modules (`app::tests::sync_*`) can assert the sequence
/// without spawning a real tmux subprocess.
pub mod test_support {
    use std::sync::Mutex;

    use super::PaneResizer;

    /// Records every `resize` invocation as `(session, cols, rows)`.
    /// Backed by `Mutex` (not `RefCell`) so the `Send + Sync` bound
    /// on `PaneResizer` is satisfiable for parity with `PaneSource`.
    #[derive(Debug, Default)]
    pub struct MockPaneResizer {
        pub calls: Mutex<Vec<(String, u16, u16)>>,
    }

    impl PaneResizer for MockPaneResizer {
        fn resize(&self, session: &str, cols: u16, rows: u16) {
            self.calls
                .lock()
                .unwrap()
                .push((session.to_string(), cols, rows));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_area_for_typical_terminal_without_approvals() {
        // 120×40: Agents=28 wide, right-stack=92 wide, Detail=24 rows
        // (3/5 of 40), Mailbox=16 rows (2/5 of 40).
        let total = Rect::new(0, 0, 120, 40);
        let detail = triptych_detail_area(total, false).unwrap();
        assert_eq!(detail.x, 28);
        assert_eq!(detail.y, 0);
        assert_eq!(detail.width, 92);
        assert_eq!(detail.height, 24);
    }

    #[test]
    fn detail_area_with_approvals_stripe_loses_one_row() {
        // Same 120×40 with the approvals stripe: stripe takes y=0
        // height=1, body starts at y=1 with height=39; Detail is
        // 3/5 of 39 = 23 rows, starting at y=1.
        let total = Rect::new(0, 0, 120, 40);
        let detail = triptych_detail_area(total, true).unwrap();
        assert_eq!(detail.x, 28);
        assert_eq!(detail.y, 1);
        assert_eq!(detail.width, 92);
        assert_eq!(detail.height, 23);
    }

    #[test]
    fn detail_area_returns_none_on_zero_dimension() {
        assert!(triptych_detail_area(Rect::new(0, 0, 0, 40), false).is_none());
        assert!(triptych_detail_area(Rect::new(0, 0, 120, 0), false).is_none());
    }

    #[test]
    fn detail_area_returns_none_when_sidebar_consumes_everything() {
        // Width 28 (or less) is exactly consumed by the Agents
        // sidebar; the right-stack has zero width, so Detail does too.
        let total = Rect::new(0, 0, 28, 40);
        assert!(triptych_detail_area(total, false).is_none());
    }

    #[test]
    fn should_sync_returns_true_on_first_call() {
        let cache = HashMap::new();
        assert!(should_sync(&cache, "t-hello-mgr", (92, 24)));
    }

    #[test]
    fn should_sync_returns_false_when_size_unchanged() {
        let mut cache = HashMap::new();
        cache.insert("t-hello-mgr".to_string(), (92, 24));
        assert!(!should_sync(&cache, "t-hello-mgr", (92, 24)));
    }

    #[test]
    fn should_sync_returns_true_when_size_differs() {
        let mut cache = HashMap::new();
        cache.insert("t-hello-mgr".to_string(), (92, 24));
        // Width changed.
        assert!(should_sync(&cache, "t-hello-mgr", (100, 24)));
        // Height changed.
        assert!(should_sync(&cache, "t-hello-mgr", (92, 25)));
    }

    #[test]
    fn should_sync_treats_different_sessions_independently() {
        let mut cache = HashMap::new();
        cache.insert("t-hello-mgr".to_string(), (92, 24));
        // Same size, different session → first-sync, should fire.
        assert!(should_sync(&cache, "t-hello-dev", (92, 24)));
    }

    use super::test_support::MockPaneResizer;

    #[test]
    fn mock_resizer_records_calls() {
        let m = MockPaneResizer::default();
        m.resize("t-a", 100, 30);
        m.resize("t-b", 80, 20);
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], ("t-a".to_string(), 100, 30));
        assert_eq!(calls[1], ("t-b".to_string(), 80, 20));
    }

    /// The CI guard for #312: pins the production verb. T-199/#210's
    /// tests asserted the sync *decision* via `MockPaneResizer` but
    /// never the *verb*, so a `resize-window`→`resize-pane` slip is
    /// invisible to them — exactly how #312 recurred. This runs in the
    /// default suite and fails the moment the verb regresses.
    #[test]
    fn resize_argv_is_resize_window_never_resize_pane() {
        let argv = super::resize_window_argv("t-hello-mgr", 92, 24);
        assert_eq!(
            argv[0], "resize-window",
            "MUST be `resize-window`: `resize-pane` silently no-ops on \
             the sole pane of a clientless detached session and reopens \
             #312"
        );
        assert_ne!(argv[0], "resize-pane", "the #312 regression verb");
        assert_eq!(
            argv,
            ["resize-window", "-t", "t-hello-mgr", "-x", "92", "-y", "24"].map(str::to_string)
        );
    }

    /// Empirical #312 repro, codified. `#[ignore]` because — unlike the
    /// rest of this crate's hermetic mock tests — it spawns a real
    /// `tmux` server; run with `cargo test -- --ignored` on a tmux
    /// host. Proves the production resizer actually shrinks a clientless
    /// session created the way `team-core::supervisor` creates it (the
    /// real-effect check #210 lacked). Against the pre-fix `resize-pane`
    /// code this fails: the window stays 200×50.
    #[test]
    #[ignore = "spawns a real tmux server; run with --ignored on a tmux host"]
    fn resize_window_actually_shrinks_a_clientless_session() {
        let session = "t312-regression-probe";
        let kill = || {
            let _ = Command::new("tmux")
                .args(["kill-session", "-t", session])
                .status();
        };
        kill();
        // Mirror team-core::supervisor: detached, clientless, 200×50.
        let created = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-x",
                "200",
                "-y",
                "50",
                "-s",
                session,
                "sh",
                "-c",
                "while :; do sleep 5; done",
            ])
            .status();
        if !matches!(created, Ok(s) if s.success()) {
            // No usable tmux in this environment — nothing to assert.
            return;
        }

        TmuxPaneResizer.resize(session, 80, 24);

        let out = Command::new("tmux")
            .args([
                "display-message",
                "-p",
                "-t",
                session,
                "#{window_width}x#{window_height}",
            ])
            .output()
            .expect("tmux display-message");
        let geom = String::from_utf8_lossy(&out.stdout).trim().to_string();
        kill();

        assert_eq!(
            geom, "80x24",
            "resizer did not shrink the clientless window (got `{geom}`) \
             — the resize-pane regression (#312)"
        );
    }
}
