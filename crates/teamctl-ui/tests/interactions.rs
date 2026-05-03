//! Keystroke-driven integration tests for the TUI.
//!
//! `Harness` builds an `App` together with mock collaborators
//! (`MockMessageSender`, `MockApprovalDecider`, `EmptyMailbox`) once
//! at construction time so subsequent dispatches spy through the
//! same instances. T-079 sub-tickets layer real coverage on top of
//! the shorthands defined here — keep this file ergonomic.
//!
//! `dispatch_key` / `dispatch_key_mods` mirror the inline
//! `dispatch(app, ev)` shape from `app.rs` unit tests, lifted into
//! the integration layer through the now-public `app::handle_event`.
//!
//! Snapshots intentionally aren't asserted here — those live in
//! `tests/snapshots.rs`. This file pins state transitions; the
//! snapshot file pins what they render to.
//!
//! ## Convention
//!
//! Set up state via direct method calls (`h.app.dismiss_splash()`,
//! `h.app.replace_team(...)`, `h.app.select_next()`); exercise the
//! verb under test via `dispatch_key` so the production
//! `handle_event` routing is what's actually being asserted. That
//! keeps the keystroke surface area narrow and the test signal
//! sharp — a regression in `handle_event` shows up as a single
//! failing dispatch, not a wall of setup that has to be untangled.
//!
//! ## Adding a test
//!
//! Negative shape (state-transition only):
//!
//! ```ignore
//! let mut h = Harness::new();
//! h.app.replace_team(fixture_team("t", vec![synth_agent("t:m", AgentState::Running, 0, 0)]));
//! h.app.dismiss_splash();
//! h.dispatch_key(KeyCode::Char('@'));        // open DM compose
//! assert_eq!(h.app.stage, Stage::ComposeModal);
//! assert!(h.sender.dm_calls.lock().unwrap().is_empty());
//! ```
//!
//! Affirmative shape (recorder captured the call):
//!
//! ```ignore
//! // …open compose, type body, fire send chord…
//! let calls = h.sender.dm_calls.lock().unwrap();
//! assert_eq!(calls.len(), 1, "exactly one send fired");
//! assert_eq!(calls[0].0, "t:m", "DM target is the focused agent");
//! assert_eq!(calls[0].1, "hello", "captured body matches what was typed");
//! ```

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use team_core::supervisor::AgentState;
use teamctl_ui::app::{self, App, Stage};
use teamctl_ui::approvals::test_support::MockApprovalDecider;
use teamctl_ui::compose::test_support::MockMessageSender;
use teamctl_ui::data::{AgentInfo, TeamSnapshot};
use teamctl_ui::mailbox::{MailboxSource, MessageRow};
use teamctl_ui::triptych::Pane;

/// Mailbox source that returns no rows for any query. T-079-A
/// mocks compose + approvals via the published `test_support`
/// modules; the mailbox surface only needs an inert read-side here,
/// so we keep the type local until a future sub-ticket needs the
/// full-fat recorder.
#[derive(Default)]
pub struct EmptyMailbox;

impl MailboxSource for EmptyMailbox {
    fn inbox(&self, _agent_id: &str, _after_id: i64) -> anyhow::Result<Vec<MessageRow>> {
        Ok(Vec::new())
    }
    fn channel_feed(&self, _agent_id: &str, _after_id: i64) -> anyhow::Result<Vec<MessageRow>> {
        Ok(Vec::new())
    }
    fn wire(&self, _project_id: &str, _after_id: i64) -> anyhow::Result<Vec<MessageRow>> {
        Ok(Vec::new())
    }
}

/// Harness binds an `App` to its mock collaborators so every
/// `dispatch_key` reaches the same recorders. Construct with
/// `Harness::new()`; seed a team via `app.replace_team(...)` when
/// the test needs roster state.
pub struct Harness {
    pub app: App,
    pub sender: MockMessageSender,
    pub decider: MockApprovalDecider,
    pub mailbox: EmptyMailbox,
}

impl Harness {
    /// Build a fresh App + default mocks. `NO_COLOR` is set so any
    /// downstream snapshot capture stays monochrome (matches the
    /// `tests/snapshots.rs` convention).
    pub fn new() -> Self {
        std::env::set_var("NO_COLOR", "1");
        Self {
            app: App::new(),
            sender: MockMessageSender::default(),
            decider: MockApprovalDecider::default(),
            mailbox: EmptyMailbox,
        }
    }

    /// Dispatch a single key (no modifiers) through the production
    /// `handle_event` routing, with the harness's mocks captured.
    pub fn dispatch_key(&mut self, code: KeyCode) {
        self.dispatch_key_mods(code, KeyModifiers::NONE);
    }

    /// Dispatch a single key with modifiers — for `Ctrl+W`,
    /// `Shift+Tab`, etc.
    pub fn dispatch_key_mods(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let ev = Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        app::handle_event(
            &mut self.app,
            ev,
            &self.decider,
            &self.sender,
            &self.mailbox,
        );
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

/// Build an `AgentInfo` row from a `project:agent` id string —
/// lifted from `tests/snapshots.rs::synth_agent` so test authors
/// don't have to assemble the struct by hand.
pub fn synth_agent(id: &str, state: AgentState, unread: u32, pending: u32) -> AgentInfo {
    let (project, agent) = id.split_once(':').unwrap_or(("p", id));
    AgentInfo {
        id: id.into(),
        agent: agent.into(),
        project: project.into(),
        tmux_session: format!("t-{}-{}", project, agent),
        state,
        unread_mail: unread,
        pending_approvals: pending,
        is_manager: false,
    }
}

/// Build a `TeamSnapshot` with a fixture root path. Lifted from
/// `tests/snapshots.rs::fixture_team`.
pub fn fixture_team(team_name: &str, agents: Vec<AgentInfo>) -> TeamSnapshot {
    TeamSnapshot {
        root: std::path::PathBuf::from("/fixture"),
        team_name: team_name.into(),
        agents,
        channels: Vec::new(),
    }
}

// ── Demonstrator tests ──────────────────────────────────────────

#[test]
fn splash_dismisses_on_any_key() {
    // Splash is the boot stage; any keypress drops to Triptych. The
    // production handler routes every Splash-stage key through
    // `dismiss_splash()` regardless of code, so a single space is
    // enough to pin the contract.
    let mut h = Harness::new();
    assert_eq!(h.app.stage, Stage::Splash);
    h.dispatch_key(KeyCode::Char(' '));
    assert_eq!(h.app.stage, Stage::Triptych);
}

#[test]
fn tab_cycles_focus_through_panes() {
    // Tab walks Roster → Detail → Mailbox → Roster uniformly. The
    // wrap back to Roster is load-bearing — it's the bug T-074
    // shipped a fix for, and the integration test layer needs to
    // catch any future regression.
    let mut h = Harness::new();
    h.app.dismiss_splash();
    assert_eq!(h.app.focused_pane, Pane::Roster);

    h.dispatch_key(KeyCode::Tab);
    assert_eq!(h.app.focused_pane, Pane::Detail);

    h.dispatch_key(KeyCode::Tab);
    assert_eq!(h.app.focused_pane, Pane::Mailbox);

    h.dispatch_key(KeyCode::Tab);
    assert_eq!(
        h.app.focused_pane,
        Pane::Roster,
        "Tab from Mailbox wraps back to Roster"
    );
}

#[test]
fn compose_modal_opens_via_at_key_without_sending() {
    // Wires the MockMessageSender end-to-end: a fixture team is
    // seeded, the operator picks an agent, `@` opens DM compose
    // pointed at that agent. Crucially, opening the modal does NOT
    // send — the recorder must still be empty. Subsequent T-079-B
    // tests will type a body + Enter and assert that the same
    // recorder captures the call.
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    h.app.dismiss_splash();
    // Cursor lands on the first agent after replace_team; advance
    // once so the DM target is dev1, not manager.
    h.app.select_next();

    h.dispatch_key(KeyCode::Char('@'));

    assert_eq!(h.app.stage, Stage::ComposeModal);
    assert!(
        h.app.compose_target.is_some(),
        "compose_target seeded by `@`"
    );
    assert!(
        h.sender.dm_calls.lock().unwrap().is_empty(),
        "opening the modal must not trigger a send"
    );
    assert!(
        h.sender.broadcast_calls.lock().unwrap().is_empty(),
        "opening the modal must not trigger a broadcast"
    );
}

// ── T-079-B: DM compose flow coverage ───────────────────────────

/// Type a body into an open compose modal, one character at a time
/// through the public dispatcher. Each char goes through
/// `handle_event` → `editor.apply_key`, exactly mirroring what the
/// run loop sees from a real terminal.
fn type_body(h: &mut Harness, body: &str) {
    for c in body.chars() {
        h.dispatch_key(KeyCode::Char(c));
    }
}

/// Set up the canonical DM-flow fixture: a two-agent team with the
/// roster cursor parked on `dev1` and the splash dismissed. Returns
/// the harness so each test can drive the verb under test.
fn dm_compose_setup() -> Harness {
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    h.app.dismiss_splash();
    h.app.select_next(); // park on dev1, not manager
    h
}

#[test]
fn dm_compose_sends_via_send_chord_with_focused_target_and_typed_body() {
    // The project-owner-reported affirmative path: open DM compose
    // for the focused agent, type a body, fire the send chord. The
    // mock sender must capture exactly one DM with the right
    // recipient and body. The send chord we drive here is the one a
    // real terminal can deliver — the historical `Ctrl+Enter` is
    // unreachable on standard terminals (xterm / Terminal.app /
    // tmux strip the Ctrl modifier on Enter), so the affirmative
    // path the operator actually uses is `Alt+Enter`.
    let mut h = dm_compose_setup();
    h.dispatch_key(KeyCode::Char('@'));
    assert_eq!(h.app.stage, Stage::ComposeModal);

    type_body(&mut h, "ready for review");
    h.dispatch_key_mods(KeyCode::Enter, KeyModifiers::ALT);

    let calls = h.sender.dm_calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one DM should fire");
    assert_eq!(calls[0].0, "writing:dev1", "DM target is the focused agent");
    assert_eq!(
        calls[0].1, "ready for review",
        "body matches what was typed"
    );
    assert_eq!(
        h.app.stage,
        Stage::Triptych,
        "successful send closes the modal"
    );
    assert!(
        h.app.compose_target.is_none(),
        "compose target cleared on close"
    );
}

#[test]
fn dm_compose_blank_body_surfaces_error_and_does_not_send() {
    // Hitting send with no body must NOT fire the sender — the
    // modal stays open with `compose_error` populated so the
    // operator sees why nothing went out.
    let mut h = dm_compose_setup();
    h.dispatch_key(KeyCode::Char('@'));
    h.dispatch_key_mods(KeyCode::Enter, KeyModifiers::ALT);

    assert!(
        h.sender.dm_calls.lock().unwrap().is_empty(),
        "blank body must not reach the sender"
    );
    assert_eq!(
        h.app.stage,
        Stage::ComposeModal,
        "modal stays open on blank-body error"
    );
    assert!(
        h.app
            .compose_error
            .as_deref()
            .is_some_and(|e| e.contains("empty")),
        "compose_error should explain the no-send: got {:?}",
        h.app.compose_error
    );
}

#[test]
fn dm_compose_esc_esc_cancels_without_sending() {
    // Esc-Esc closes the modal without invoking the sender. The
    // editor consumes the first Esc to leave Insert mode, the
    // second Esc to fire EditorAction::Cancel.
    let mut h = dm_compose_setup();
    h.dispatch_key(KeyCode::Char('@'));
    type_body(&mut h, "draft");

    h.dispatch_key(KeyCode::Esc);
    h.dispatch_key(KeyCode::Esc);

    assert!(
        h.sender.dm_calls.lock().unwrap().is_empty(),
        "Esc-Esc must not fire the sender"
    );
    assert_eq!(h.app.stage, Stage::Triptych, "Esc-Esc closes the modal");
    assert!(
        h.app.compose_target.is_none(),
        "compose target cleared on cancel"
    );
}

#[test]
fn dm_compose_multi_line_body_is_sent_with_embedded_newline() {
    // Plain Enter inserts a newline (multi-line composition); the
    // send chord then ships the joined body. The captured payload
    // must preserve the embedded `\n` so downstream rendering
    // doesn't collapse the operator's intent.
    let mut h = dm_compose_setup();
    h.dispatch_key(KeyCode::Char('@'));
    type_body(&mut h, "line one");
    h.dispatch_key(KeyCode::Enter); // newline within Insert mode
    type_body(&mut h, "line two");
    h.dispatch_key_mods(KeyCode::Enter, KeyModifiers::ALT);

    let calls = h.sender.dm_calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "multi-line send fires exactly once");
    assert_eq!(
        calls[0].1, "line one\nline two",
        "newline preserved in body"
    );
}

#[test]
fn dm_compose_target_follows_roster_selection_after_cancel() {
    // Cancel mid-compose, advance the roster cursor, reopen
    // compose. The new target must be the freshly-focused agent —
    // a stale target would silently DM the wrong person.
    let mut h = dm_compose_setup();
    // First open: cursor parked on dev1, cancel.
    h.dispatch_key(KeyCode::Char('@'));
    h.dispatch_key(KeyCode::Esc);
    h.dispatch_key(KeyCode::Esc);
    assert_eq!(h.app.stage, Stage::Triptych);

    // Advance to manager (wraps from dev1 → manager via select_next).
    h.app.select_next();
    h.dispatch_key(KeyCode::Char('@'));
    type_body(&mut h, "ping");
    h.dispatch_key_mods(KeyCode::Enter, KeyModifiers::ALT);

    let calls = h.sender.dm_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0, "writing:manager",
        "DM follows the new roster selection, not the prior cancelled target"
    );
}
