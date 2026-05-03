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
use teamctl_ui::app::{self, render_to_buffer, App, Stage};
use teamctl_ui::approvals::test_support::MockApprovalDecider;
use teamctl_ui::approvals::{Approval, Decision};
use teamctl_ui::compose::test_support::MockMessageSender;
use teamctl_ui::data::{AgentInfo, TeamSnapshot};
use teamctl_ui::mailbox::{MailboxSource, MessageRow};
use teamctl_ui::triptych::{MainLayout, Pane};

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

// ── Layout-switch coverage (T-079-C) ────────────────────────────
//
// Affirmative paths, return paths, cross-layout transitions, edge
// cases (focused pane, modal stages). The render-side assertions
// catch the "state flips but the view doesn't follow" failure mode
// — the project-owner-reported bug shape.

fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(area.x + x, area.y + y)];
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn ctrl_w_switches_triptych_to_wall() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    assert_eq!(h.app.layout, MainLayout::Triptych);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::Wall);
}

#[test]
fn ctrl_w_returns_wall_to_triptych() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::Wall);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::Triptych);
}

#[test]
fn ctrl_m_switches_triptych_to_mailbox_first() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    assert_eq!(h.app.layout, MainLayout::Triptych);

    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::MailboxFirst);
}

#[test]
fn ctrl_m_returns_mailbox_first_to_triptych() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::MailboxFirst);

    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::Triptych);
}

#[test]
fn ctrl_m_from_wall_switches_to_mailbox_first() {
    // Cross-layout: operator in Wall hits Ctrl+M, expects to land
    // in MailboxFirst directly without a Triptych pit-stop.
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::Wall);

    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::MailboxFirst);
}

#[test]
fn ctrl_w_from_mailbox_first_switches_to_wall() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::MailboxFirst);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::Wall);
}

#[test]
fn ctrl_w_with_mailbox_pane_focused_still_switches_layout() {
    // Layout-switch must work regardless of which pane has focus —
    // Ctrl+W is a layout-level chord, not a pane-level one.
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key(KeyCode::Tab);
    h.dispatch_key(KeyCode::Tab);
    assert_eq!(h.app.focused_pane, Pane::Mailbox);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(h.app.layout, MainLayout::Wall);
}

#[test]
fn compose_modal_blocks_layout_switch() {
    // The compose modal owns input — Ctrl+W must NOT bypass the
    // editor and flip the underlying main-view layout. Operator
    // would see a confused state ("modal is up but layout
    // changed") otherwise.
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    h.app.dismiss_splash();
    h.dispatch_key(KeyCode::Char('@'));
    assert_eq!(h.app.stage, Stage::ComposeModal);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(
        h.app.layout,
        MainLayout::Triptych,
        "compose modal owns input — layout must not flip underneath"
    );
}

#[test]
fn quit_confirm_overlay_blocks_layout_switch() {
    let mut h = Harness::new();
    h.app.dismiss_splash();
    h.dispatch_key(KeyCode::Char('q'));
    assert_eq!(h.app.stage, Stage::QuitConfirm);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(
        h.app.layout,
        MainLayout::Triptych,
        "quit-confirm overlay must not be bypassed by layout chord"
    );
}

#[test]
fn rendered_buffer_reflects_wall_after_ctrl_w() {
    // The named bug shape: state flips but the rendered view
    // doesn't follow. Triptych shows ROSTER + MAILBOX pane titles;
    // Wall is a tile grid with no such pane chrome. If `app.layout`
    // is Wall but `render_to_buffer` still emits ROSTER/MAILBOX,
    // the user sees "switching layouts doesn't work."
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    h.app.dismiss_splash();

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::Wall);

    let s = buffer_to_string(&render_to_buffer(&h.app, 120, 30));
    assert!(
        !s.contains("ROSTER"),
        "Wall buffer must not render the Triptych ROSTER pane title:\n{s}"
    );
    assert!(
        !s.contains("MAILBOX"),
        "Wall buffer must not render the Triptych MAILBOX pane title:\n{s}"
    );
}

#[test]
fn rendered_buffer_reflects_mailbox_first_after_ctrl_m() {
    // Same shape for the MailboxFirst layout — the failure mode
    // covered here is the project-owner-reported "switching
    // layouts doesn't work" surface for Ctrl+M.
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    h.app.dismiss_splash();

    h.dispatch_key_mods(KeyCode::Char('m'), KeyModifiers::CONTROL);
    assert_eq!(h.app.layout, MainLayout::MailboxFirst);

    let s = buffer_to_string(&render_to_buffer(&h.app, 120, 30));
    assert!(
        !s.contains("ROSTER"),
        "MailboxFirst buffer must not render the Triptych ROSTER pane title:\n{s}"
    );
    assert!(
        !s.contains("DETAIL"),
        "MailboxFirst buffer must not render the Triptych DETAIL pane title:\n{s}"
    );
}

#[test]
fn ctrl_shift_w_still_toggles_wall_layout() {
    // The project-owner-reported "switching layouts doesn't work"
    // surface. With CapsLock on, or with Shift held alongside
    // Ctrl, crossterm reports `KeyCode::Char('W')` (uppercase)
    // with `CONTROL` (and possibly `SHIFT`) modifiers. The current
    // routing only matches lowercase `Char('w')`, so the chord
    // dies silently and the operator sees a no-op. Layout chord
    // must accept either casing — the user's intent is "Ctrl
    // plus the W key," not "the lowercase glyph 'w'."
    let mut h = Harness::new();
    h.app.dismiss_splash();

    h.dispatch_key_mods(
        KeyCode::Char('W'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );

    assert_eq!(h.app.layout, MainLayout::Wall);
}

#[test]
fn ctrl_shift_m_still_toggles_mailbox_first_layout() {
    // Mirror of the Ctrl+W case for Ctrl+M — same root cause,
    // same fix surface.
    let mut h = Harness::new();
    h.app.dismiss_splash();

    h.dispatch_key_mods(
        KeyCode::Char('M'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );

    assert_eq!(h.app.layout, MainLayout::MailboxFirst);
}

#[test]
fn ctrl_w_with_detail_split_open_arms_chord_not_layout() {
    // Documented PR-UI-7 behaviour pinned: when there's at least
    // one detail split, Ctrl+W arms the close-split chord prefix
    // rather than toggling the Wall layout. The chord follows
    // with `q` (close focused split) or `o` (close others).
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    h.app.dismiss_splash();
    h.dispatch_key_mods(KeyCode::Char('|'), KeyModifiers::CONTROL);
    assert_eq!(h.app.detail_splits.len(), 1);

    h.dispatch_key_mods(KeyCode::Char('w'), KeyModifiers::CONTROL);

    assert_eq!(
        h.app.pending_chord,
        Some(KeyCode::Char('w')),
        "Ctrl+W with splits arms the chord prefix"
    );
    assert_eq!(
        h.app.layout,
        MainLayout::Triptych,
        "Ctrl+W with splits must not flip layout (chord wins)"
    );
}

// ── Approvals modal coverage (T-079-D) ──────────────────────────
//
// Drive: `a` to enter when there's a pending row, j/k (or arrows)
// to navigate, `y` / `Y` approve, `Shift+N` deny (asymmetric chord
// is load-bearing — see T-074 bug 4 fix at app::handle_event), Esc
// or `q` exit. Asserts target the `MockApprovalDecider`'s captured
// (id, kind, note) tuple so the routing reaches the right row.

/// Build an `Approval` with stable defaults — tests vary the id /
/// action / summary fields and pin everything else.
fn approval(id: i64, action: &str, summary: &str) -> Approval {
    Approval {
        id,
        project_id: "writing".into(),
        agent_id: "writing:manager".into(),
        action: action.into(),
        summary: summary.into(),
        payload_json: String::new(),
    }
}

/// Seed a fixture team + N pending approvals + dismiss splash.
/// Returns the harness so each test drives the verb under test.
fn approvals_setup(approvals: Vec<Approval>) -> Harness {
    let mut h = Harness::new();
    h.app.replace_team(fixture_team(
        "writing",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    h.app.dismiss_splash();
    h.app.replace_approvals(approvals);
    h
}

#[test]
fn a_key_enters_approvals_modal_when_pending_non_empty() {
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);

    h.dispatch_key(KeyCode::Char('a'));

    assert_eq!(h.app.stage, Stage::ApprovalsModal);
    assert_eq!(h.app.selected_approval, 0);
}

#[test]
fn a_key_is_no_op_when_no_pending_approvals() {
    // `enter_approvals_modal` early-returns on empty queue (app.rs
    // L428). Without a pending row there's nothing to triage, so
    // the chord is silent rather than opening an empty modal.
    let mut h = approvals_setup(vec![]);

    h.dispatch_key(KeyCode::Char('a'));

    assert_eq!(h.app.stage, Stage::Triptych);
}

#[test]
fn y_approves_focused_row_via_decider() {
    // Affirmative path: open modal, hit `y`, the decider receives
    // (focused-row-id, Approve, ""). Modal auto-closes because the
    // queue is now empty.
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key(KeyCode::Char('y'));

    let calls = h.decider.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (7, Decision::Approve, String::new()));
    assert_eq!(
        h.app.stage,
        Stage::Triptych,
        "queue emptied — modal auto-closes"
    );
}

#[test]
fn capital_y_also_approves_focused_row() {
    // Approve accepts both `y` and `Y` — matches the QuitConfirm
    // loose convention. Pin both casings so the chord stays
    // discoverable for CapsLock/Shift operators.
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key_mods(KeyCode::Char('Y'), KeyModifiers::SHIFT);

    let calls = h.decider.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, Decision::Approve);
}

#[test]
fn shift_n_denies_focused_row_via_decider() {
    // Deny is the destructive side — Shift-gated (`N` only) per
    // T-074 bug 4 fix. Drives the asymmetric-chord contract.
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key_mods(KeyCode::Char('N'), KeyModifiers::SHIFT);

    let calls = h.decider.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (7, Decision::Deny, String::new()));
}

#[test]
fn lowercase_n_does_not_deny_destructive_chord_is_shift_gated() {
    // Counter-test of the asymmetric chord: stray lowercase `n` in
    // the modal must not fire deny. Operator hammering keys can't
    // accidentally drop an approval on the floor.
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key(KeyCode::Char('n'));

    assert!(
        h.decider.calls.lock().unwrap().is_empty(),
        "lowercase n must not reach the decider"
    );
    assert_eq!(
        h.app.stage,
        Stage::ApprovalsModal,
        "modal stays open on no-op key"
    );
}

#[test]
fn j_advances_focused_approval_and_k_retreats() {
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
        approval(9, "merge", "third"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));
    assert_eq!(h.app.selected_approval, 0);

    h.dispatch_key(KeyCode::Char('j'));
    assert_eq!(h.app.selected_approval, 1);

    h.dispatch_key(KeyCode::Char('j'));
    assert_eq!(h.app.selected_approval, 2);

    h.dispatch_key(KeyCode::Char('k'));
    assert_eq!(h.app.selected_approval, 1);
}

#[test]
fn down_and_up_arrows_navigate_like_j_and_k() {
    // Arrows mirror j/k for non-vim operators. Both shapes route
    // through the same cycle_approval_next/prev — pin parity.
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key(KeyCode::Down);
    assert_eq!(h.app.selected_approval, 1);

    h.dispatch_key(KeyCode::Up);
    assert_eq!(h.app.selected_approval, 0);
}

#[test]
fn j_wraps_at_end_of_queue() {
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));
    h.dispatch_key(KeyCode::Char('j'));
    assert_eq!(h.app.selected_approval, 1);

    h.dispatch_key(KeyCode::Char('j'));

    assert_eq!(
        h.app.selected_approval, 0,
        "j from last row wraps back to first"
    );
}

#[test]
fn k_wraps_at_start_of_queue() {
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));
    assert_eq!(h.app.selected_approval, 0);

    h.dispatch_key(KeyCode::Char('k'));

    assert_eq!(h.app.selected_approval, 1, "k from first row wraps to last");
}

#[test]
fn esc_closes_approvals_modal_without_decision() {
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key(KeyCode::Esc);

    assert_eq!(h.app.stage, Stage::Triptych);
    assert!(h.decider.calls.lock().unwrap().is_empty());
    assert_eq!(h.app.pending_approvals.len(), 1, "queue intact after Esc");
}

#[test]
fn q_closes_approvals_modal_without_decision() {
    // `q` is the alternative exit chord (matches QuitConfirm/Help
    // overlay convention). No quit-confirm overlay should surface
    // — the modal owns input.
    let mut h = approvals_setup(vec![approval(7, "publish", "post the brief")]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key(KeyCode::Char('q'));

    assert_eq!(h.app.stage, Stage::Triptych);
    assert!(h.decider.calls.lock().unwrap().is_empty());
}

#[test]
fn navigate_then_approve_routes_correct_row_id_to_decider() {
    // The end-to-end test the ticket flags: 3-row queue, navigate
    // to the middle row, approve. Decider must receive the middle
    // id (8), not the first (7) or last (9). Catches "selected_
    // approval drifts but apply_decision uses index 0" regressions.
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
        approval(9, "merge", "third"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));
    h.dispatch_key(KeyCode::Char('j'));
    assert_eq!(h.app.selected_approval, 1);

    h.dispatch_key(KeyCode::Char('y'));

    let calls = h.decider.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0, 8,
        "decider received the focused-row id (middle), not the head"
    );
    assert_eq!(calls[0].1, Decision::Approve);
    assert_eq!(
        h.app.pending_approvals.len(),
        2,
        "approved row removed from local queue optimistically"
    );
    assert_eq!(
        h.app.stage,
        Stage::ApprovalsModal,
        "modal stays open while queue still has rows"
    );
}

#[test]
fn deny_followed_by_approve_routes_each_to_distinct_rows() {
    // Two-step end-to-end: Shift-N denies the head row (id 7),
    // selected_approval clamps onto the new head (id 8), `y` then
    // approves it. Decider must show one Deny on 7 and one Approve
    // on 8 — pins the optimistic-removal-and-clamp path through
    // `apply_decision`.
    let mut h = approvals_setup(vec![
        approval(7, "publish", "first"),
        approval(8, "deploy", "second"),
    ]);
    h.dispatch_key(KeyCode::Char('a'));

    h.dispatch_key_mods(KeyCode::Char('N'), KeyModifiers::SHIFT);
    h.dispatch_key(KeyCode::Char('y'));

    let calls = h.decider.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], (7, Decision::Deny, String::new()));
    assert_eq!(calls[1], (8, Decision::Approve, String::new()));
    assert_eq!(
        h.app.stage,
        Stage::Triptych,
        "queue empty after both decisions"
    );
}
