use super::*;
use crate::approvals::{Approval, ApprovalSource, BrokerApprovalSource, Decision};
use crate::data::TeamSnapshot;
use crate::mailbox::{BrokerMailboxSource, MailboxInputKind, MailboxTab, MessageRow};
use crate::triptych::{MainLayout, Pane};
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::{layout::Rect, Terminal};
use std::time::{Duration, Instant};

use crate::data::AgentInfo;
use crossterm::event::{KeyEvent, KeyEventState, KeyModifiers};
use team_core::supervisor::AgentState;

mod regressions;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

/// Noop decider for tests that don't exercise approve/deny.
struct NoopDecider;
impl crate::approvals::ApprovalDecider for NoopDecider {
    fn decide(
        &self,
        _root: &std::path::Path,
        _id: i64,
        _kind: crate::approvals::Decision,
        _note: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Noop sender for tests that don't exercise compose-send.
struct NoopSender;
impl crate::compose::MessageSender for NoopSender {
    fn send_dm(&self, _root: &std::path::Path, _agent: &str, _body: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn broadcast(
        &self,
        _root: &std::path::Path,
        _channel: &str,
        _body: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Mailbox source that returns nothing — refresh_mailbox after
/// a successful send becomes a no-op.
struct EmptyMailbox;
impl crate::mailbox::MailboxSource for EmptyMailbox {
    fn inbox(&self, _id: &str, _after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        Ok(Vec::new())
    }
    fn sent(&self, _id: &str, _after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        Ok(Vec::new())
    }
    fn channel_feed(
        &self,
        _id: &str,
        _after: i64,
    ) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        Ok(Vec::new())
    }
    fn wire(&self, _id: &str, _after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        Ok(Vec::new())
    }
}

/// Boilerplate-free dispatcher for tests not exercising the
/// decision / send paths.
fn dispatch(app: &mut App, ev: Event) {
    super::handle_event(
        app,
        ev,
        &NoopDecider,
        &NoopSender,
        &EmptyMailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
}

fn agent(id: &str, state: AgentState) -> AgentInfo {
    AgentInfo {
        id: id.into(),
        agent: id
            .split_once(':')
            .map(|(_, a)| a.to_string())
            .unwrap_or_default(),
        project: id
            .split_once(':')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default(),
        tmux_session: format!("t-{}", id.replace(':', "-")),
        state,
        unread_mail: 0,
        pending_approvals: 0,
        is_manager: false,
        display_name: None,
        rate_limit_resets_at: None,
        last_activity_at: None,
        reports_to: None,
    }
}

pub fn fixture_team(agents: Vec<AgentInfo>) -> TeamSnapshot {
    TeamSnapshot {
        root: std::path::PathBuf::from("/fixture"),
        team_name: "fixture".into(),
        agents,
        channels: Vec::new(),
    }
}

#[test]
fn splash_dismissed_by_any_key() {
    let mut app = App::new();
    assert_eq!(app.stage, Stage::Splash);
    dispatch(&mut app, key(KeyCode::Char(' ')));
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn tab_cycles_panes_uniformly_and_wraps_through_mailbox() {
    // T-074 bug 6: Tab cycles pane focus only — Roster → Detail
    // → Mailbox → Roster — at every step. The previous "Tab
    // cycles tabs once focused on mailbox" shape stranded
    // operators inside the mailbox; this test pins the corrected
    // uniform cycle so a future refactor can't reintroduce the
    // dead-end.
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.focused_pane, Pane::Roster);
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focused_pane, Pane::Detail);
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focused_pane, Pane::Mailbox);
    assert_eq!(
        app.mailbox_tab,
        MailboxTab::Inbox,
        "Tab into mailbox does NOT touch the active mailbox tab"
    );
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(
        app.focused_pane,
        Pane::Roster,
        "Tab from mailbox wraps to roster, not into mailbox subtabs"
    );
    assert_eq!(
        app.mailbox_tab,
        MailboxTab::Inbox,
        "mailbox tab still untouched"
    );
}

#[test]
fn arrow_keys_walk_mailbox_tabs_when_mailbox_focused() {
    // T-124: Right/Left arrows are the mailbox-tab walker
    // (more discoverable than the prior `[`/`]` chord). Gated
    // on mailbox being the focused pane so the arrows stay
    // unsurprising in every other context.
    let mut app = App::new();
    app.dismiss_splash();
    // Walk into mailbox via Tab.
    dispatch(&mut app, key(KeyCode::Tab));
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focused_pane, Pane::Mailbox);
    assert_eq!(app.mailbox_tab, MailboxTab::Inbox);

    // #462: two user tabs → Right toggles Inbox ↔ Sent and wraps.
    dispatch(&mut app, key(KeyCode::Right));
    assert_eq!(app.mailbox_tab, MailboxTab::Sent);
    dispatch(&mut app, key(KeyCode::Right));
    assert_eq!(app.mailbox_tab, MailboxTab::Inbox, "→ wraps");

    dispatch(&mut app, key(KeyCode::Left));
    assert_eq!(app.mailbox_tab, MailboxTab::Sent, "← walks back");
}

#[test]
fn arrow_keys_no_op_when_mailbox_not_focused() {
    // The arrows must not surprise an operator scrolling the
    // roster — gate is load-bearing.
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.focused_pane, Pane::Roster);
    let initial = app.mailbox_tab;
    dispatch(&mut app, key(KeyCode::Right));
    dispatch(&mut app, key(KeyCode::Left));
    assert_eq!(
        app.mailbox_tab, initial,
        "←/→ from non-mailbox panes must not flip the active tab"
    );
}

#[test]
fn brackets_no_longer_cycle_mailbox_tabs() {
    // T-124 regression: `[` / `]` were the previous binding;
    // hard-swap means they are now fully inert in the mailbox
    // pane. Pin the no-op so a future binding can't quietly
    // re-introduce the old chord.
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Tab));
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focused_pane, Pane::Mailbox);
    let initial = app.mailbox_tab;

    dispatch(&mut app, key(KeyCode::Char(']')));
    dispatch(&mut app, key(KeyCode::Char('[')));
    assert_eq!(
        app.mailbox_tab, initial,
        "`[` / `]` must no longer cycle mailbox tabs (T-124 hard-swap)",
    );
}

#[test]
fn q_opens_confirm_then_n_cancels() {
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('q')));
    assert_eq!(app.stage, Stage::QuitConfirm);
    dispatch(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.stage, Stage::Triptych);
    assert!(app.running, "n must not exit");
}

#[test]
fn q_then_y_exits() {
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('q')));
    dispatch(&mut app, key(KeyCode::Char('y')));
    assert!(!app.running);
}

#[test]
fn esc_cancels_quit_confirm() {
    let mut app = App::new();
    app.dismiss_splash();
    app.enter_quit_confirm();
    dispatch(&mut app, key(KeyCode::Esc));
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn render_does_not_panic_at_minimal_size() {
    let app = App::new();
    let _ = render_to_buffer(&app, 20, 8);
}

#[test]
fn render_does_not_panic_at_huge_size() {
    let app = App::new();
    let _ = render_to_buffer(&app, 240, 80);
}

#[test]
fn select_next_wraps_through_team() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
        agent("p:c", AgentState::Running),
    ]));
    assert_eq!(app.selected_agent, Some(0));
    app.select_next();
    assert_eq!(app.selected_agent, Some(1));
    app.select_next();
    assert_eq!(app.selected_agent, Some(2));
    app.select_next();
    assert_eq!(app.selected_agent, Some(0)); // wraps
}

#[test]
fn select_prev_wraps_at_top() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.selected_agent = Some(0);
    app.select_prev();
    assert_eq!(app.selected_agent, Some(1));
}

#[test]
fn select_no_op_on_empty_team() {
    let mut app = App::new();
    app.select_next();
    assert_eq!(app.selected_agent, None);
    app.select_prev();
    assert_eq!(app.selected_agent, None);
}

#[test]
fn replace_team_preserves_selection_when_agent_still_present() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.selected_agent = Some(1);
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Stopped), // same id, new state
    ]));
    assert_eq!(app.selected_agent, Some(1), "selection follows the id");
}

#[test]
fn replace_team_resets_selection_when_agent_disappears() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:gone", AgentState::Running),
    ]));
    app.selected_agent = Some(1);
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    assert_eq!(app.selected_agent, Some(0), "falls back to first agent");
}

#[test]
fn switching_agent_resets_mailbox_buffers() {
    // The mailbox cursors are per-agent context; switching to a
    // new agent must clear them so we don't skip historical
    // rows that landed before the new agent's first refresh.
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.mailbox.extend(
        crate::mailbox::MailboxTab::Inbox,
        vec![crate::mailbox::MessageRow {
            id: 7,
            sender: "p:b".into(),
            recipient: "p:a".into(),
            text: "hi".into(),
            sent_at: 0.0,
        }],
    );
    assert_eq!(app.mailbox.inbox.len(), 1);
    assert_eq!(app.mailbox.inbox_after, 7);
    // Move selection to p:b — different agent id, mailbox resets.
    app.select_next();
    assert_eq!(app.selected_agent_id().as_deref(), Some("p:b"));
    assert!(app.mailbox.inbox.is_empty());
    assert_eq!(app.mailbox.inbox_after, 0);
}

/// Tiny single-call mailbox stub for the refresh-fanout test —
/// keeps the assertion local without depending on
/// `mailbox::tests::MockMailboxSource` (which lives behind a
/// private `tests` module).
struct TripleFilterMock {
    inbox: Vec<crate::mailbox::MessageRow>,
    sent: Vec<crate::mailbox::MessageRow>,
    channel: Vec<crate::mailbox::MessageRow>,
    wire: Vec<crate::mailbox::MessageRow>,
    calls: std::sync::Mutex<Vec<(&'static str, String, i64)>>,
}
impl crate::mailbox::MailboxSource for TripleFilterMock {
    fn inbox(&self, id: &str, after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        self.calls.lock().unwrap().push(("inbox", id.into(), after));
        Ok(self.inbox.clone())
    }
    fn sent(&self, id: &str, after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        self.calls.lock().unwrap().push(("sent", id.into(), after));
        Ok(self.sent.clone())
    }
    fn channel_feed(
        &self,
        id: &str,
        after: i64,
    ) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        self.calls
            .lock()
            .unwrap()
            .push(("channel", id.into(), after));
        Ok(self.channel.clone())
    }
    fn wire(&self, id: &str, after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
        self.calls.lock().unwrap().push(("wire", id.into(), after));
        Ok(self.wire.clone())
    }
}

#[test]
fn refresh_mailbox_fans_out_to_four_filters() {
    use crate::mailbox::MessageRow;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    let mock = TripleFilterMock {
        inbox: vec![MessageRow {
            id: 1,
            sender: "p:b".into(),
            recipient: "p:a".into(),
            text: "dm".into(),
            sent_at: 0.0,
        }],
        sent: vec![MessageRow {
            id: 4,
            sender: "p:a".into(),
            recipient: "p:b".into(),
            text: "outgoing dm".into(),
            sent_at: 0.0,
        }],
        channel: vec![MessageRow {
            id: 2,
            sender: "p:b".into(),
            recipient: "channel:p:editorial".into(),
            text: "ch".into(),
            sent_at: 0.0,
        }],
        wire: vec![MessageRow {
            id: 3,
            sender: "p:b".into(),
            recipient: "channel:p:all".into(),
            text: "wire".into(),
            sent_at: 0.0,
        }],
        calls: std::sync::Mutex::new(Vec::new()),
    };
    super::refresh_mailbox(&mut app, &mock);
    assert_eq!(app.mailbox.inbox.len(), 1);
    assert_eq!(app.mailbox.sent.len(), 1);
    assert_eq!(app.mailbox.channel.len(), 1);
    assert_eq!(app.mailbox.wire.len(), 1);
    let calls = mock.calls.lock().unwrap();
    // The selected agent is p:a (auto-set by replace_team to
    // index 0); the wire filter takes the project id `p`.
    assert!(calls.contains(&("inbox", "p:a".into(), 0)));
    assert!(calls.contains(&("sent", "p:a".into(), 0)));
    assert!(calls.contains(&("channel", "p:a".into(), 0)));
    assert!(calls.contains(&("wire", "p".into(), 0)));
}

fn ap(id: i64) -> crate::approvals::Approval {
    crate::approvals::Approval {
        id,
        project_id: "p".into(),
        agent_id: "p:m".into(),
        action: "publish".into(),
        summary: format!("approval #{id}"),
        payload_json: String::new(),
    }
}

#[test]
fn has_pending_approvals_tracks_replace_calls() {
    let mut app = App::new();
    assert!(!app.has_pending_approvals());
    app.replace_approvals(vec![ap(1), ap(2)]);
    assert!(app.has_pending_approvals());
    app.replace_approvals(vec![]);
    assert!(!app.has_pending_approvals());
}

#[test]
fn enter_approvals_modal_no_op_when_queue_empty() {
    let mut app = App::new();
    app.dismiss_splash();
    app.enter_approvals_modal();
    assert_eq!(app.stage, Stage::Triptych, "no pending → no modal");
}

#[test]
fn a_chord_opens_modal_when_pending() {
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(1), ap(2)]);
    dispatch(&mut app, key(KeyCode::Char('a')));
    assert_eq!(app.stage, Stage::ApprovalsModal);
    assert_eq!(app.selected_approval, 0);
}

#[test]
fn modal_cycle_jk_walks_approvals() {
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(1), ap(2), ap(3)]);
    app.enter_approvals_modal();
    dispatch(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.selected_approval, 1);
    dispatch(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.selected_approval, 2);
    dispatch(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.selected_approval, 0, "wraps");
    dispatch(&mut app, key(KeyCode::Char('k')));
    assert_eq!(app.selected_approval, 2, "k wraps too");
}

#[test]
fn capital_y_routes_approve_through_decider() {
    use crate::approvals::test_support::MockApprovalDecider;
    let dec = MockApprovalDecider::default();
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(7), ap(8)]);
    app.enter_approvals_modal();
    super::handle_event(
        &mut app,
        key(KeyCode::Char('Y')),
        &dec,
        &NoopSender,
        &EmptyMailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    let calls = dec.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, 7);
    assert_eq!(calls[0].1, crate::approvals::Decision::Approve);
    // Optimistic local removal — approval id 7 dropped.
    assert_eq!(app.pending_approvals.len(), 1);
    assert_eq!(app.pending_approvals[0].id, 8);
}

#[test]
fn capital_n_routes_deny_through_decider() {
    use crate::approvals::test_support::MockApprovalDecider;
    let dec = MockApprovalDecider::default();
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(7)]);
    app.enter_approvals_modal();
    super::handle_event(
        &mut app,
        key(KeyCode::Char('N')),
        &dec,
        &NoopSender,
        &EmptyMailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    let calls = dec.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, crate::approvals::Decision::Deny);
    // Queue empty after the only approval resolves → modal closes.
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn esc_closes_approvals_modal() {
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(1)]);
    app.enter_approvals_modal();
    dispatch(&mut app, key(KeyCode::Esc));
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn lowercase_y_routes_approve_through_decider() {
    // T-074 bug 4: discoverable approve. Most operators try
    // lowercase first; the modal must accept it on the
    // approve (low-risk) side. Deny stays Shift-gated.
    use crate::approvals::test_support::MockApprovalDecider;
    let dec = MockApprovalDecider::default();
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(7)]);
    app.enter_approvals_modal();
    super::handle_event(
        &mut app,
        key(KeyCode::Char('y')),
        &dec,
        &NoopSender,
        &EmptyMailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    let calls = dec.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, crate::approvals::Decision::Approve);
}

#[test]
fn lowercase_n_does_not_deny() {
    // Asymmetry guard: deny is destructive — `n` lowercase must
    // NOT fire the decider. A future "symmetric loose" refactor
    // would silently regress the destructive-deny Shift-gate;
    // this test pins it.
    use crate::approvals::test_support::MockApprovalDecider;
    let dec = MockApprovalDecider::default();
    let mut app = App::new();
    app.dismiss_splash();
    app.replace_approvals(vec![ap(7)]);
    app.enter_approvals_modal();
    super::handle_event(
        &mut app,
        key(KeyCode::Char('n')),
        &dec,
        &NoopSender,
        &EmptyMailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    assert!(
        dec.calls.lock().unwrap().is_empty(),
        "lowercase n must not route through the decider"
    );
    assert_eq!(
        app.stage,
        Stage::ApprovalsModal,
        "stale lowercase n leaves the modal open"
    );
}

#[test]
fn shift_tab_cycles_panes_backward() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.focused_pane, Pane::Roster);
    // Shift+Tab from Roster → Mailbox (the "back out of mailbox"
    // direction's mirror).
    dispatch(&mut app, key(KeyCode::BackTab));
    assert_eq!(app.focused_pane, Pane::Mailbox);
    // Some terminals send Tab + SHIFT instead of BackTab.
    dispatch(&mut app, key_with(KeyCode::Tab, KeyModifiers::SHIFT));
    assert_eq!(app.focused_pane, Pane::Detail);
}

#[test]
fn at_chord_opens_compose_dm_to_focused_agent() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("writing:manager", AgentState::Running),
        agent("writing:dev1", AgentState::Running),
    ]));
    app.dismiss_splash();
    app.select_next();
    dispatch(&mut app, key(KeyCode::Char('@')));
    assert_eq!(app.stage, Stage::ComposeModal);
    match app.compose_target.as_ref() {
        Some(crate::compose::ComposeTarget::Dm { agent_id, .. }) => {
            assert_eq!(agent_id, "writing:dev1");
        }
        other => panic!("expected DM target, got {other:?}"),
    }
}

#[test]
fn bang_chord_opens_compose_broadcast_to_all_channel() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent(
        "writing:manager",
        AgentState::Running,
    )]));
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('!')));
    assert_eq!(app.stage, Stage::ComposeModal);
    match app.compose_target.as_ref() {
        Some(crate::compose::ComposeTarget::Broadcast { channel_id, .. }) => {
            assert_eq!(channel_id, "writing:all");
        }
        other => panic!("expected Broadcast target, got {other:?}"),
    }
}

#[test]
fn send_routes_dm_through_mock_sender() {
    use crate::compose::test_support::MockMessageSender;
    let sender = MockMessageSender::default();
    let mailbox = EmptyMailbox;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent(
        "writing:dev1",
        AgentState::Running,
    )]));
    app.dismiss_splash();
    app.enter_compose_dm_for_focused();
    for c in "ship it".chars() {
        super::handle_event(
            &mut app,
            key(KeyCode::Char(c)),
            &NoopDecider,
            &sender,
            &mailbox,
            &crate::keysender::test_support::MockKeySender::default(),
        );
    }
    super::handle_event(
        &mut app,
        key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    let calls = sender.dm_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "writing:dev1");
    assert_eq!(calls[0].1, "ship it");
    assert_eq!(app.stage, Stage::Triptych, "modal closes on send");
}

#[test]
fn esc_esc_cancels_compose_without_send() {
    use crate::compose::test_support::MockMessageSender;
    let sender = MockMessageSender::default();
    let mailbox = EmptyMailbox;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent(
        "writing:dev1",
        AgentState::Running,
    )]));
    app.dismiss_splash();
    app.enter_compose_dm_for_focused();
    for c in "draft".chars() {
        super::handle_event(
            &mut app,
            key(KeyCode::Char(c)),
            &NoopDecider,
            &sender,
            &mailbox,
            &crate::keysender::test_support::MockKeySender::default(),
        );
    }
    super::handle_event(
        &mut app,
        key(KeyCode::Esc),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    super::handle_event(
        &mut app,
        key(KeyCode::Esc),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    assert_eq!(app.stage, Stage::Triptych);
    assert!(sender.dm_calls.lock().unwrap().is_empty());
}

#[test]
fn send_failure_surfaces_error_inline_keeps_modal_open() {
    use crate::compose::test_support::MockMessageSender;
    let sender = MockMessageSender::default();
    *sender.fail_next.lock().unwrap() = Some("rate limit".into());
    let mailbox = EmptyMailbox;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent(
        "writing:dev1",
        AgentState::Running,
    )]));
    app.dismiss_splash();
    app.enter_compose_dm_for_focused();
    super::handle_event(
        &mut app,
        key(KeyCode::Char('x')),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    super::handle_event(
        &mut app,
        key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    assert_eq!(app.stage, Stage::ComposeModal, "modal stays open on err");
    assert!(app
        .compose_error
        .as_deref()
        .unwrap_or_default()
        .contains("rate limit"));
}

fn channel(id: &str, project: &str) -> crate::data::ChannelInfo {
    crate::data::ChannelInfo {
        id: id.into(),
        name: id
            .rsplit_once(':')
            .map(|(_, n)| n.to_string())
            .unwrap_or_default(),
        project_id: project.into(),
    }
}

fn fixture_team_with_channels(
    agents: Vec<AgentInfo>,
    channels: Vec<crate::data::ChannelInfo>,
) -> TeamSnapshot {
    TeamSnapshot {
        root: std::path::PathBuf::from("/fixture"),
        team_name: "fixture".into(),
        agents,
        channels,
    }
}

#[test]
fn ctrl_w_toggles_wall_layout() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.layout, MainLayout::Triptych);
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.layout, MainLayout::Wall);
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.layout, MainLayout::Triptych);
}

#[test]
fn ctrl_m_toggles_mailbox_first_layout() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(
        &mut app,
        key_with(KeyCode::Char('m'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.layout, MainLayout::MailboxFirst);
    dispatch(
        &mut app,
        key_with(KeyCode::Char('m'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.layout, MainLayout::Triptych);
}

#[test]
fn wall_scroll_pages_through_overflow_agents() {
    let mut app = App::new();
    let mut agents: Vec<_> = (1..=10)
        .map(|i| agent(&format!("p:agent-{i:02}"), AgentState::Running))
        .collect();
    // managers-first sort would otherwise reorder; mark all as workers.
    for a in agents.iter_mut() {
        a.is_manager = false;
    }
    app.replace_team(fixture_team(agents));
    app.dismiss_splash();
    app.toggle_wall_layout();
    assert_eq!(app.wall_scroll, 0);
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 4);
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 8);
    // Past 10-1 = 9; cap blocks 12.
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 8, "scroll capped at last full window");
    app.wall_scroll_up();
    assert_eq!(app.wall_scroll, 4);
}

#[test]
fn ctrl_pipe_adds_detail_split_capped_at_four() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.dismiss_splash();
    for _ in 0..6 {
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
    }
    assert_eq!(app.detail_splits.len(), 4, "split count capped at 4");
}

#[test]
fn ctrl_q_closes_focused_split() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    dispatch(
        &mut app,
        key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
    );
    dispatch(
        &mut app,
        key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.detail_splits.len(), 2);
    dispatch(
        &mut app,
        key_with(KeyCode::Char('Q'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.detail_splits.len(), 1);
}

#[test]
fn ctrl_hjkl_cycles_splits() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    for _ in 0..3 {
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
    }
    assert_eq!(app.selected_split, 3);
    dispatch(
        &mut app,
        key_with(KeyCode::Char('l'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.selected_split, 0, "wraps");
    dispatch(
        &mut app,
        key_with(KeyCode::Char('h'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.selected_split, 3);
}

#[test]
fn wall_scroll_at_exactly_cap_agents_does_not_scroll() {
    // PR-UI-6 fixup (qa Gap 1a): with exactly WALL_TILE_CAP=4
    // agents the entire team fits in one window — scrolling
    // is a no-op in both directions. Pinning this catches a
    // future `<` → `<=` slip in `wall_scroll_down`.
    let mut app = App::new();
    let agents: Vec<_> = (1..=4)
        .map(|i| agent(&format!("p:agent-{i}"), AgentState::Running))
        .collect();
    app.replace_team(fixture_team(agents));
    app.dismiss_splash();
    app.toggle_wall_layout();
    assert_eq!(app.wall_scroll, 0);
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 0, "exactly-cap should not advance");
    app.wall_scroll_up();
    assert_eq!(app.wall_scroll, 0);
}

#[test]
fn wall_scroll_at_cap_plus_one_advances_then_stops() {
    // PR-UI-6 fixup (qa Gap 1b): exactly 5 agents → 4 fit in
    // window-0, the 5th lives at window-4. One scroll
    // advances; the next caps. Pins the off-by-one between 4
    // and 5 agents.
    let mut app = App::new();
    let agents: Vec<_> = (1..=5)
        .map(|i| agent(&format!("p:agent-{i}"), AgentState::Running))
        .collect();
    app.replace_team(fixture_team(agents));
    app.dismiss_splash();
    app.toggle_wall_layout();
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 4, "first scroll exposes agent 5");
    app.wall_scroll_down();
    assert_eq!(app.wall_scroll, 4, "second scroll caps; nothing past");
}

#[test]
fn esc_in_picker_dismisses_overlay_only_keeps_modal_open() {
    // PR-UI-6 fixup (Q6 dev2 review + qa Gap 3): Esc inside
    // the broadcast picker should close the picker overlay
    // and return to the editor in its current state — NOT
    // close the whole compose modal. Editor's Esc-Esc
    // already handles cancel-the-modal.
    let mut app = App::new();
    app.replace_team(fixture_team_with_channels(
        vec![agent("writing:manager", AgentState::Running)],
        vec![
            channel("writing:all", "writing"),
            channel("writing:editorial", "writing"),
        ],
    ));
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('!')));
    assert!(app.compose_picker_open);
    assert_eq!(app.stage, Stage::ComposeModal);
    dispatch(&mut app, key(KeyCode::Esc));
    assert!(!app.compose_picker_open, "picker dismissed");
    assert_eq!(app.stage, Stage::ComposeModal, "compose modal stays open");
}

#[test]
fn send_routes_broadcast_through_mock_sender_via_picker() {
    // PR-UI-6 fixup (qa Gap 4): the broadcast path needs the
    // same MockMessageSender pin the DM path got in PR-UI-5.
    // Pins both per-channel-correct-id (picker selection
    // flows through to the send call) AND routes-through-
    // `broadcast()`-not-`send()` (no DM call recorded).
    use crate::compose::test_support::MockMessageSender;
    let sender = MockMessageSender::default();
    let mailbox = EmptyMailbox;
    let mut app = App::new();
    app.replace_team(fixture_team_with_channels(
        vec![agent("writing:manager", AgentState::Running)],
        vec![
            channel("writing:all", "writing"),
            channel("writing:editorial", "writing"),
            channel("writing:critique", "writing"),
        ],
    ));
    app.dismiss_splash();
    // Open picker, walk to channel index 1 (`editorial`),
    // confirm, type a body, Ctrl+Enter to send.
    super::handle_event(
        &mut app,
        key(KeyCode::Char('!')),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    super::handle_event(
        &mut app,
        key(KeyCode::Char('j')),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    super::handle_event(
        &mut app,
        key(KeyCode::Enter),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    for c in "ship docs".chars() {
        super::handle_event(
            &mut app,
            key(KeyCode::Char(c)),
            &NoopDecider,
            &sender,
            &mailbox,
            &crate::keysender::test_support::MockKeySender::default(),
        );
    }
    super::handle_event(
        &mut app,
        key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
        &NoopDecider,
        &sender,
        &mailbox,
        &crate::keysender::test_support::MockKeySender::default(),
    );
    let dm_calls = sender.dm_calls.lock().unwrap().clone();
    let bcast_calls = sender.broadcast_calls.lock().unwrap().clone();
    assert!(dm_calls.is_empty(), "broadcast must not route via send_dm");
    assert_eq!(bcast_calls.len(), 1);
    assert_eq!(
        bcast_calls[0].0, "writing:editorial",
        "channel id from picker selection"
    );
    assert_eq!(bcast_calls[0].1, "ship docs");
    assert_eq!(app.stage, Stage::Triptych, "modal closes on send");
}

#[test]
fn bang_chord_opens_picker_when_channels_available() {
    let mut app = App::new();
    app.replace_team(fixture_team_with_channels(
        vec![agent("writing:manager", AgentState::Running)],
        vec![
            channel("writing:all", "writing"),
            channel("writing:editorial", "writing"),
            channel("writing:critique", "writing"),
        ],
    ));
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('!')));
    assert_eq!(app.stage, Stage::ComposeModal);
    assert!(app.compose_picker_open);
    // Walk the picker.
    dispatch(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.compose_picker_index, 1);
    // Confirm pulls into compose target.
    dispatch(&mut app, key(KeyCode::Enter));
    assert!(!app.compose_picker_open, "picker closes on confirm");
    match app.compose_target.as_ref() {
        Some(crate::compose::ComposeTarget::Broadcast { channel_id, .. }) => {
            assert_eq!(channel_id, "writing:editorial");
        }
        other => panic!("expected Broadcast target, got {other:?}"),
    }
}

#[test]
fn mailbox_first_layout_seeds_channel_selection_on_entry() {
    let mut app = App::new();
    app.replace_team(fixture_team_with_channels(
        vec![agent("writing:manager", AgentState::Running)],
        vec![
            channel("writing:all", "writing"),
            channel("writing:editorial", "writing"),
        ],
    ));
    app.dismiss_splash();
    assert!(app.selected_channel.is_none());
    app.toggle_mailbox_first_layout();
    assert_eq!(app.selected_channel, Some(0));
}

#[test]
fn help_overlay_opens_on_question_mark_closes_on_esc() {
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('?')));
    assert_eq!(app.stage, Stage::HelpOverlay);
    dispatch(&mut app, key(KeyCode::Esc));
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn tutorial_opens_on_t_advances_and_closes() {
    let mut app = App::new();
    app.dismiss_splash();
    dispatch(&mut app, key(KeyCode::Char('t')));
    assert_eq!(app.stage, Stage::Tutorial);
    assert_eq!(app.tutorial_step, 0);
    // Any non-Esc/back key advances.
    dispatch(&mut app, key(KeyCode::Char(' ')));
    assert_eq!(app.tutorial_step, 1);
    // `k` walks back.
    dispatch(&mut app, key(KeyCode::Char('k')));
    assert_eq!(app.tutorial_step, 0);
    // Esc closes from any step.
    dispatch(&mut app, key(KeyCode::Esc));
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn tutorial_walk_back_at_step_zero_is_no_op() {
    // qa Gap C fold: pin the chosen behaviour for `k`/`Up`/`p`
    // at step 0 — saturating decrement keeps `tutorial_step`
    // at 0 rather than wrapping. Any future shift to
    // wrap-to-end would break this test, which is the point.
    let mut app = App::new();
    app.dismiss_splash();
    app.enter_tutorial();
    assert_eq!(app.tutorial_step, 0);
    dispatch(&mut app, key(KeyCode::Char('k')));
    assert_eq!(app.tutorial_step, 0, "step-0 walk-back is no-op");
    // The walk-back keypress must NOT close the tutorial
    // either — the Stage stays.
    assert_eq!(app.stage, Stage::Tutorial);
}

#[test]
fn ctrl_pipe_adds_vertical_split_ctrl_minus_adds_horizontal() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    dispatch(
        &mut app,
        key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
    );
    dispatch(
        &mut app,
        key_with(KeyCode::Char('-'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.detail_splits.len(), 2);
    assert_eq!(app.detail_splits[0].1, SplitOrientation::Vertical);
    assert_eq!(app.detail_splits[1].1, SplitOrientation::Horizontal);
}

#[test]
fn ctrl_w_q_chord_prefix_closes_focused_split() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    // Two splits — `Ctrl+W` arms only when there's something
    // to close.
    dispatch(
        &mut app,
        key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
    );
    dispatch(
        &mut app,
        key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
    );
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.pending_chord, Some(KeyCode::Char('w')));
    // Plain `q` (no modifier) is now interpreted as the
    // chord-prefix follow-up — close split, NOT quit.
    dispatch(&mut app, key(KeyCode::Char('q')));
    assert_eq!(app.detail_splits.len(), 1);
    assert_eq!(app.stage, Stage::Triptych, "must not enter quit confirm");
    assert_eq!(app.pending_chord, None, "chord cleared");
}

#[test]
fn ctrl_w_o_chord_keeps_only_focused_split() {
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    for _ in 0..3 {
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
    }
    // Focus the middle split.
    app.selected_split = 1;
    let kept_id = app.detail_splits[0].0.clone();
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    dispatch(&mut app, key(KeyCode::Char('o')));
    assert!(app.detail_splits.is_empty());
    assert_eq!(app.selected_agent_id().as_deref(), Some(kept_id.as_str()));
    assert_eq!(app.selected_split, 0);
}

#[test]
fn add_detail_split_saturates_at_four_with_explicit_4_and_5_calls() {
    // qa Gap 2 fold: pin the cap explicitly. Reaching exactly
    // 4 must stick; the 5th call must be a no-op (not panic,
    // not silently grow). If `add_detail_split` ever returns
    // a Result, this test catches the silent-success regression.
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    for _ in 0..4 {
        app.add_detail_split();
    }
    assert_eq!(app.detail_splits.len(), 4);
    let snapshot_len = app.detail_splits.len();
    app.add_detail_split();
    assert_eq!(app.detail_splits.len(), snapshot_len, "5th call rejected");
}

#[test]
fn replace_approvals_clamps_selection_in_range() {
    let mut app = App::new();
    app.replace_approvals(vec![ap(1), ap(2), ap(3)]);
    app.selected_approval = 2;
    // Approval id 3 resolved out-of-band; new snapshot has 2 rows.
    app.replace_approvals(vec![ap(1), ap(2)]);
    assert_eq!(app.selected_approval, 1, "clamps to last index");
}

#[test]
fn arrow_keys_navigate_only_when_roster_focused() {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.dismiss_splash();
    // Focused pane is Roster → arrow cycles selection.
    app.selected_agent = Some(0);
    dispatch(&mut app, key(KeyCode::Down));
    assert_eq!(app.selected_agent, Some(1));
    // Cycle to Detail → arrow no longer touches selection.
    app.cycle_focus();
    dispatch(&mut app, key(KeyCode::Down));
    assert_eq!(
        app.selected_agent,
        Some(1),
        "non-roster focus ignores arrows"
    );
}

// ---- T-108 stream-keys mode -------------------------------------------

/// Spin up a Triptych-stage app with one agent selected and the
/// detail pane focused — the standard precondition for entering
/// stream-keys mode.
fn stream_keys_fixture() -> App {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    app.cycle_focus(); // Roster → Detail
    assert_eq!(app.focused_pane, Pane::Detail);
    assert_eq!(app.selected_agent, Some(0));
    app
}

fn stream_dispatch(
    app: &mut App,
    ev: Event,
    key_sender: &crate::keysender::test_support::MockKeySender,
) {
    super::handle_event(
        app,
        ev,
        &NoopDecider,
        &NoopSender,
        &EmptyMailbox,
        key_sender,
    );
}

#[test]
fn ctrl_e_enters_stream_keys_when_detail_focused() {
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = stream_keys_fixture();
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &ks,
    );
    assert_eq!(app.stage, Stage::StreamKeys);
    assert!(
        ks.calls.lock().unwrap().is_empty(),
        "the activation chord itself never forwards a keystroke"
    );
}

#[test]
fn ctrl_e_no_op_when_detail_not_focused() {
    // Activation gate: stream-mode never triggers from Roster /
    // Mailbox focus, so a stray `Ctrl+E` while scrolling the
    // roster doesn't yank the operator into a modal they didn't
    // ask for.
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    assert_eq!(app.focused_pane, Pane::Roster);
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &ks,
    );
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn ctrl_e_no_op_when_no_agent_selected() {
    // No target session → entering stream-mode would type into
    // the void. The guard short-circuits.
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = App::new();
    app.dismiss_splash();
    app.cycle_focus(); // Detail
    assert_eq!(app.selected_agent, None);
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &ks,
    );
    assert_eq!(app.stage, Stage::Triptych);
}

#[test]
fn esc_forwards_to_pane_in_stream_keys() {
    // T-374: Esc is NOT the exit chord — it forwards to the
    // agent's tmux pane like any other key, so Claude Code (where
    // Esc is load-bearing) receives it. Exit is `Ctrl+E` now.
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    assert_eq!(app.stage, Stage::StreamKeys);
    let ks = MockKeySender::default();
    stream_dispatch(&mut app, key_with(KeyCode::Esc, KeyModifiers::NONE), &ks);
    assert_eq!(
        app.stage,
        Stage::StreamKeys,
        "Esc does NOT exit stream-keys"
    );
    let calls = ks.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "Esc forwards as one keystroke");
    assert_eq!(calls[0].0, "t-p-a");
    assert_eq!(calls[0].1.args, vec!["Escape".to_string()]);
}

#[test]
fn ctrl_e_exits_stream_keys() {
    // T-374: Ctrl+E is the symmetric exit toggle (same chord
    // that enters the mode). It must exit, not forward.
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    assert_eq!(app.stage, Stage::StreamKeys);
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &ks,
    );
    assert_eq!(app.stage, Stage::Triptych);
    assert!(
        ks.calls.lock().unwrap().is_empty(),
        "Ctrl+E is the exit chord — it must not forward as a keystroke"
    );
}

#[test]
fn stream_mode_forwards_printable_chars_to_target_session() {
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    let ks = MockKeySender::default();
    for c in "hi".chars() {
        stream_dispatch(&mut app, key(KeyCode::Char(c)), &ks);
    }
    let calls = ks.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "one tmux send-keys per keystroke");
    // Target session = the focused agent's tmux_session (set by
    // the fixture to `t-p-a`).
    assert_eq!(calls[0].0, "t-p-a");
    assert_eq!(calls[0].1.args, vec!["-l".to_string(), "h".to_string()]);
    assert_eq!(calls[1].1.args, vec!["-l".to_string(), "i".to_string()]);
}

#[test]
fn stream_mode_passes_ctrl_c_through_to_agent() {
    // Issue #108 design point: Ctrl+C is shell-SIGINT semantics,
    // not a stream-mode escape. Pin the contract so a future
    // "intercept Ctrl+C as bail" refactor doesn't regress it.
    use crate::keysender::test_support::MockKeySender;
    use crossterm::event::KeyModifiers;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &ks,
    );
    assert_eq!(app.stage, Stage::StreamKeys, "Ctrl+C does NOT exit");
    let calls = ks.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.args, vec!["C-c".to_string()]);
}

#[test]
fn stream_mode_forwards_enter_and_arrows() {
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    let ks = MockKeySender::default();
    stream_dispatch(&mut app, key(KeyCode::Enter), &ks);
    stream_dispatch(&mut app, key(KeyCode::Up), &ks);
    let calls = ks.calls.lock().unwrap();
    assert_eq!(calls[0].1.args, vec!["Enter".to_string()]);
    assert_eq!(calls[1].1.args, vec!["Up".to_string()]);
}

#[test]
fn stream_target_session_uses_focused_split_when_present() {
    // Splits change which agent the operator is "looking at."
    // The selected_split index drives the focus ring in
    // render_detail_splits; stream_target_session must mirror
    // that so typing lands in the right pane.
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.dismiss_splash();
    app.cycle_focus(); // Detail
    app.selected_agent = Some(0);
    // Manually push a split for `p:b` and focus it.
    app.detail_splits
        .push(("p:b".into(), crate::app::SplitOrientation::Vertical));
    app.selected_split = 1; // cell index 0 = focused agent, 1 = first split
    let target = app.stream_target_session();
    assert_eq!(
        target.as_deref(),
        Some("t-p-b"),
        "selected split's agent drives the target"
    );
}

#[test]
fn stream_mode_drops_back_when_target_session_disappears() {
    // If the team gets reloaded mid-stream and the focused
    // agent's index points off the end, the next keystroke
    // can't resolve a session. Drop back to Triptych so the
    // operator isn't silently typing into the void.
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture();
    app.enter_stream_keys();
    // Simulate the agent disappearing.
    app.selected_agent = None;
    app.team.agents.clear();
    let ks = MockKeySender::default();
    stream_dispatch(&mut app, key(KeyCode::Char('a')), &ks);
    assert_eq!(app.stage, Stage::Triptych);
    assert!(ks.calls.lock().unwrap().is_empty());
}

// ── fast-cadence focused-pane re-capture ────────────────────────

#[test]
fn recapture_focused_pane_sets_buffer_and_advances_clock() {
    // Between full 1s refreshes the run loop re-captures just the
    // focused agent's pane so the detail view stays live. It must
    // (a) push the captured lines into the detail buffer and
    // (b) advance `last_pane_refresh` so the next tick re-gates.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    assert_eq!(app.selected_agent, Some(0));
    let mock = MockPaneSource {
        lines: vec!["hello".into(), "world".into()],
        asked: std::sync::Mutex::new(Vec::new()),
        ..Default::default()
    };
    // Backdate the clock so any advance is observable.
    let before = Instant::now() - PANE_REFRESH_INTERVAL;
    app.last_pane_refresh = before;

    super::recapture_focused_pane(&mut app, &mock);

    assert_eq!(app.detail_buffer, vec!["hello", "world"]);
    // The focused agent's session — and only that one — got captured.
    assert_eq!(mock.asked.lock().unwrap().clone(), vec!["t-p-a"]);
    assert!(
        app.last_pane_refresh > before,
        "re-capture advances the fast-cadence clock"
    );
}

#[test]
fn recapture_focused_pane_no_op_when_no_agent_focused() {
    // Empty team → `focused_session()` is None, so there is nothing
    // to capture. It must not panic and must not query the source.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.selected_agent, None);
    let mock = MockPaneSource {
        lines: vec!["unused".into()],
        asked: std::sync::Mutex::new(Vec::new()),
        ..Default::default()
    };

    super::recapture_focused_pane(&mut app, &mock);

    assert!(
        mock.asked.lock().unwrap().is_empty(),
        "no focused agent → no capture call"
    );
    assert!(
        app.detail_buffer.is_empty(),
        "detail buffer untouched with no agent"
    );
}

#[test]
fn recapture_focused_pane_no_op_when_selection_cleared() {
    // A populated team but `selected_agent == None` (e.g. focus
    // dropped) is also a no-op — the source is never queried.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    app.selected_agent = None;
    let mock = MockPaneSource {
        lines: vec!["unused".into()],
        asked: std::sync::Mutex::new(Vec::new()),
        ..Default::default()
    };

    super::recapture_focused_pane(&mut app, &mock);

    assert!(mock.asked.lock().unwrap().is_empty());
    assert!(app.detail_buffer.is_empty());
}

#[test]
fn recapture_focused_pane_skips_when_activity_unchanged() {
    // #277: same focused session + unchanged activity ts → skip the
    // heavy capture; a changed ts re-captures.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    let session = app.focused_session().unwrap().to_string();
    // Activity ts 100/200 are far in the past relative to this clock,
    // so the settled-second condition (`live_ts < now_secs`) holds and
    // this exercises the cache-hit skip, not the active-second path.
    app.now_secs = 1_000.0;

    let mock = MockPaneSource {
        lines: vec!["frame-1".into()],
        activity_ts: Some(100),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock);
    assert_eq!(app.detail_buffer, vec!["frame-1"]);
    assert_eq!(mock.asked.lock().unwrap().clone(), vec![session.clone()]);
    assert_eq!(app.detail_buffer_activity, Some((session.clone(), 100)));

    // Same ts already cached → skip; source not queried, buffer kept.
    let mock2 = MockPaneSource {
        lines: vec!["frame-2-should-not-appear".into()],
        activity_ts: Some(100),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock2);
    assert!(
        mock2.asked.lock().unwrap().is_empty(),
        "unchanged activity ts → capture skipped"
    );
    assert_eq!(app.detail_buffer, vec!["frame-1"], "buffer kept");

    // ts advances → capture the new frame + reseed.
    let mock3 = MockPaneSource {
        lines: vec!["frame-3".into()],
        activity_ts: Some(200),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock3);
    assert_eq!(app.detail_buffer, vec!["frame-3"]);
    assert_eq!(mock3.asked.lock().unwrap().clone(), vec![session.clone()]);
    assert_eq!(app.detail_buffer_activity, Some((session, 200)));
}

#[test]
fn recapture_focused_pane_recaptures_during_active_second() {
    // #277 regression (PR #454): `#{window_activity}` is whole-second,
    // so an actively-changing pane — operator typing >1 key/s, or an
    // agent streaming output — reports the SAME ts for every 10Hz tick
    // within that second. The original `cached_ts == live_ts` gate
    // skipped all of them and the detail view froze at ~1Hz. The
    // settled-second gate (`live_ts < now_secs`) must re-capture while
    // the activity ts is the CURRENT second, then resume skipping once
    // the clock moves past it.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    let session = app.focused_session().unwrap().to_string();

    // The pane's last activity is happening in the current wall second,
    // and a previous tick this same second already cached a frame.
    let active_second = 1_781_343_000u64;
    app.now_secs = active_second as f64;
    app.detail_buffer_activity = Some((session.clone(), active_second));
    app.set_detail_buffer(vec!["stale-within-second".into()]);

    // Same session, same whole-second ts → the old gate would skip.
    // The settled gate must re-capture: the second isn't settled yet.
    let mock = MockPaneSource {
        lines: vec!["live-frame".into()],
        activity_ts: Some(active_second),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock);
    assert_eq!(
        mock.asked.lock().unwrap().clone(),
        vec![session.clone()],
        "activity in the current second must re-capture, not freeze at ~1Hz"
    );
    assert_eq!(app.detail_buffer, vec!["live-frame"]);

    // Clock ticks past that second with no new activity → the same
    // cached==live ts is now settled → skip (idle-CPU win preserved).
    app.now_secs = (active_second + 1) as f64;
    let mock_idle = MockPaneSource {
        lines: vec!["should-not-appear".into()],
        activity_ts: Some(active_second),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock_idle);
    assert!(
        mock_idle.asked.lock().unwrap().is_empty(),
        "settled activity ts → capture skipped"
    );
    assert_eq!(app.detail_buffer, vec!["live-frame"], "buffer kept");
}

#[test]
fn recapture_focused_pane_recaptures_on_focus_switch() {
    // #277 regression: switching focus to a different agent must
    // re-capture even when the new agent's activity ts equals what's
    // cached, otherwise the detail buffer would keep showing the
    // PREVIOUS agent's content. The `(session, ts)` cache key is what
    // prevents that stale frame.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.dismiss_splash();
    // Settled clock, so the focus-switch recapture is forced by the
    // session-key mismatch — not by the active-second path.
    app.now_secs = 1_000.0;

    app.selected_agent = Some(0);
    let sess_a = app.focused_session().unwrap().to_string();
    let mock_a = MockPaneSource {
        lines: vec!["A-frame".into()],
        activity_ts: Some(100),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock_a);
    assert_eq!(app.detail_buffer, vec!["A-frame"]);

    // Switch to B, whose activity ts is ALSO 100. Same ts, different
    // session → must capture B, not reuse A's stale frame.
    app.selected_agent = Some(1);
    let sess_b = app.focused_session().unwrap().to_string();
    assert_ne!(sess_a, sess_b);
    let mock_b = MockPaneSource {
        lines: vec!["B-frame".into()],
        activity_ts: Some(100),
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock_b);
    assert_eq!(
        mock_b.asked.lock().unwrap().clone(),
        vec![sess_b.clone()],
        "focus switch must capture the newly focused session"
    );
    assert_eq!(
        app.detail_buffer,
        vec!["B-frame"],
        "buffer holds B's content, not stale A"
    );
    assert_eq!(app.detail_buffer_activity, Some((sess_b, 100)));
}

#[test]
fn recapture_focused_pane_captures_when_activity_unreadable() {
    // #277 fallback: an unreadable activity ts (`None` — old tmux,
    // missing session) captures on every tick, the pre-#277 behaviour.
    use crate::pane::test_support::MockPaneSource;
    let mut app = App::new();
    app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
    app.dismiss_splash();
    let mock = MockPaneSource {
        lines: vec!["x".into()],
        activity_ts: None,
        ..Default::default()
    };
    super::recapture_focused_pane(&mut app, &mock);
    super::recapture_focused_pane(&mut app, &mock);
    assert_eq!(
        mock.asked.lock().unwrap().len(),
        2,
        "unreadable ts → capture every call"
    );
}

// ── Ctrl+Shift+↑/↓ stream-keys agent switch ─────────────────────

/// Two-agent variant of `stream_keys_fixture`: Detail focused,
/// agent 0 selected, already in StreamKeys mode. Used by the
/// agent-switch chord tests.
fn stream_keys_fixture_two_agents() -> App {
    let mut app = App::new();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.dismiss_splash();
    app.cycle_focus(); // Roster → Detail
    assert_eq!(app.focused_pane, Pane::Detail);
    assert_eq!(app.selected_agent, Some(0));
    app.enter_stream_keys();
    assert_eq!(app.stage, Stage::StreamKeys);
    app
}

#[test]
fn ctrl_shift_down_moves_selection_to_next_agent_no_split() {
    // With no detail split focused, the switch chord steps the
    // roster selection forward, stays in StreamKeys, and consumes
    // the chord (no stray keystroke forwarded to the pane).
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture_two_agents();
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        &ks,
    );
    assert_eq!(app.selected_agent, Some(1), "switched to next agent");
    assert_eq!(app.stage, Stage::StreamKeys, "stays in stream-keys");
    assert!(
        ks.calls.lock().unwrap().is_empty(),
        "the switch chord never forwards a keystroke"
    );
}

#[test]
fn ctrl_shift_up_moves_selection_to_prev_agent_no_split() {
    // Mirror of the Down case: Ctrl+Shift+Up steps the selection
    // backward (wrapping from agent 0 to the last agent).
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture_two_agents();
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        &ks,
    );
    assert_eq!(
        app.selected_agent,
        Some(1),
        "Up from agent 0 wraps to the last agent"
    );
    assert_eq!(app.stage, Stage::StreamKeys);
    assert!(ks.calls.lock().unwrap().is_empty());
}

#[test]
fn ctrl_shift_switch_no_op_when_split_focused() {
    // Regression guard (the bug this PR fixes): with a detail split
    // focused, `stream_target_session` routes to the split's agent,
    // not `selected_agent`. Moving the roster selection there would
    // desync the banner/detail from where keystrokes actually land,
    // so the chord is consumed as a pure no-op: selection unchanged,
    // stage unchanged, and nothing forwarded to the pane.
    use crate::keysender::test_support::MockKeySender;
    for code in [KeyCode::Up, KeyCode::Down] {
        let mut app = stream_keys_fixture_two_agents();
        // Push a split for `p:b` and focus it (cell 0 = focused
        // agent, cell 1 = first split).
        app.detail_splits
            .push(("p:b".into(), SplitOrientation::Vertical));
        app.selected_split = 1;
        let ks = MockKeySender::default();
        stream_dispatch(
            &mut app,
            key_with(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            &ks,
        );
        assert_eq!(
            app.selected_agent,
            Some(0),
            "split focused → selection must not move ({code:?})"
        );
        assert_eq!(app.stage, Stage::StreamKeys);
        assert!(
            ks.calls.lock().unwrap().is_empty(),
            "split-focused switch chord is consumed, not forwarded ({code:?})"
        );
    }
}

#[test]
fn ctrl_shift_switch_single_agent_is_no_op() {
    // Single-agent team: stepping forward wraps back to index 0,
    // so the selection is effectively unchanged. No panic, and the
    // chord still doesn't leak a keystroke to the pane.
    use crate::keysender::test_support::MockKeySender;
    let mut app = stream_keys_fixture(); // one agent, StreamKeys-capable
    app.enter_stream_keys();
    assert_eq!(app.selected_agent, Some(0));
    let ks = MockKeySender::default();
    stream_dispatch(
        &mut app,
        key_with(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        &ks,
    );
    assert_eq!(app.selected_agent, Some(0), "single agent → stays at 0");
    assert_eq!(app.stage, Stage::StreamKeys);
    assert!(ks.calls.lock().unwrap().is_empty());
}

// ── T-199: detail-pane → inner-tmux size sync ───────────────────

fn pane_sync_fixture() -> App {
    let mut app = App::new();
    app.team = fixture_team(vec![
        agent("hello:mgr", AgentState::Running),
        agent("hello:dev", AgentState::Running),
    ]);
    app.selected_agent = Some(0);
    app.stage = Stage::Triptych;
    app.layout = MainLayout::Triptych;
    app
}

#[test]
fn sync_fires_resize_on_first_frame() {
    let mut app = pane_sync_fixture();
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    let calls = resizer.calls.lock().unwrap();
    // First frame: cache empty, expect one call for the focused
    // session (mgr) at the typical 120×40 Triptych Detail rect.
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "t-hello-mgr");
    assert_eq!(calls[0].1, 90); // inner = (120 - 28 sidebar) - 2 border
    assert_eq!(calls[0].2, 21); // content excludes the two-row footer
}

#[test]
fn sync_uses_taller_detail_in_stream_keys_mode() {
    // The load-bearing wire (#459): `sync_focused_pane_size_to` must
    // read `Stage::StreamKeys` and push the taller Detail height into
    // the tmux pane — otherwise the on-screen Mailbox shrink is
    // cosmetic and the agent keeps reflowing to the old 60% height.
    // Same 120×40 terminal as `sync_fires_resize_on_first_frame`
    // (which asserts 22 rows in normal mode); stream-keys gives the
    // Mailbox a fixed Length(5) strip, so Detail is 40-5=35 → inner 33.
    let mut app = pane_sync_fixture();
    app.stage = Stage::StreamKeys;
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    let calls = resizer.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "t-hello-mgr");
    assert_eq!(calls[0].1, 90); // width unchanged: (120 - 28) - 2 border
    assert_eq!(calls[0].2, 31); // (40 - 2 footer - 5 mailbox) - 2 border
}

#[test]
fn sync_skips_when_size_unchanged() {
    let mut app = pane_sync_fixture();
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    // Two frames at identical size → only the first should fire.
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    assert_eq!(resizer.calls.lock().unwrap().len(), 1);
}

#[test]
fn sync_fires_again_when_terminal_resizes() {
    let mut app = pane_sync_fixture();
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    // Operator resized the host terminal.
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 200, 60),
        &resizer,
    );
    let calls = resizer.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, 90); // (120 - 28) - 2 border
    assert_eq!(calls[0].2, 21); // content excludes the two-row footer
    assert_eq!(calls[1].1, 170); // (200 - 28) - 2 border
                                 // Height = (3/5 of 60) - 2 border = 34.
    assert_eq!(calls[1].2, 33);
}

#[test]
fn sync_fires_on_focus_switch_to_unsynced_session() {
    let mut app = pane_sync_fixture();
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    // Operator switched focus to the dev agent.
    app.selected_agent = Some(1);
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    let calls = resizer.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "t-hello-mgr");
    assert_eq!(calls[1].0, "t-hello-dev");
}

#[test]
fn sync_is_noop_when_no_agent_focused() {
    let mut app = pane_sync_fixture();
    app.selected_agent = None;
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    assert!(resizer.calls.lock().unwrap().is_empty());
}

#[test]
fn sync_is_noop_when_layout_is_not_triptych() {
    let mut app = pane_sync_fixture();
    app.layout = MainLayout::Wall;
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    // Wall / MailboxFirst use different geometry; out of scope for
    // T-199. No tmux resize-pane should fire from this path.
    assert!(resizer.calls.lock().unwrap().is_empty());
}

#[test]
fn sync_is_noop_on_degenerate_terminal_area() {
    let mut app = pane_sync_fixture();
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    // Width is exactly the sidebar (28) → Detail rect is zero.
    sync_focused_pane_size_to(&mut app, ratatui::layout::Rect::new(0, 0, 28, 40), &resizer);
    assert!(resizer.calls.lock().unwrap().is_empty());
}

#[test]
fn sync_accounts_for_approvals_stripe_when_present() {
    let mut app = pane_sync_fixture();
    // Force the approvals-stripe path: one pending approval.
    app.pending_approvals = vec![crate::approvals::Approval {
        id: 1,
        project_id: "hello".into(),
        agent_id: "hello:dev".into(),
        action: "test".into(),
        summary: "test approval".into(),
        payload_json: String::new(),
    }];
    assert!(app.has_pending_approvals());
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    sync_focused_pane_size_to(
        &mut app,
        ratatui::layout::Rect::new(0, 0, 120, 40),
        &resizer,
    );
    let calls = resizer.calls.lock().unwrap();
    // Stripe consumes one row → Detail height 3/5 of 39 = 23 outer,
    // minus 2 border = 21 inner.
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].2, 20);
}

// T-131 PR-2: mailbox filter/search input-mode integration tests.
// Drive real key events through `handle_event` to pin the
// dispatch contract (input arms at the top of the Triptych match,
// openers gated on Pane::Mailbox).

fn app_with_mailbox_focused() -> App {
    let mut app = App::new();
    app.dismiss_splash();
    // Cycle focus to the Mailbox pane (Roster → Detail → Mailbox).
    app.cycle_focus();
    app.cycle_focus();
    assert_eq!(app.focused_pane, Pane::Mailbox);
    app
}

#[test]
fn f_opens_filter_input_when_mailbox_focused() {
    let mut app = app_with_mailbox_focused();
    assert!(app.mailbox_input_mode.is_none());
    dispatch(&mut app, key(KeyCode::Char('f')));
    assert_eq!(app.mailbox_input_mode, Some(MailboxInputKind::Filter));
}

#[test]
fn slash_opens_search_input_when_mailbox_focused() {
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('/')));
    assert_eq!(app.mailbox_input_mode, Some(MailboxInputKind::Search));
}

#[test]
fn f_does_not_open_filter_when_roster_focused() {
    // The opener is Pane::Mailbox-gated so it stays unsurprising
    // in other panes (where `f` has no meaning today, but the
    // guard keeps us out of trouble if it later picks up one).
    let mut app = App::new();
    app.dismiss_splash();
    assert_eq!(app.focused_pane, Pane::Roster);
    dispatch(&mut app, key(KeyCode::Char('f')));
    assert!(app.mailbox_input_mode.is_none());
}

#[test]
fn typing_into_filter_input_mutates_active_tab_buffer() {
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('f')));
    dispatch(&mut app, key(KeyCode::Char('a')));
    dispatch(&mut app, key(KeyCode::Char('d')));
    dispatch(&mut app, key(KeyCode::Char('a')));
    assert_eq!(app.mailbox.filter_text(app.mailbox_tab), "ada");
    // Sibling tab's filter must remain empty (per-tab independence).
    assert_eq!(app.mailbox.filter_text(MailboxTab::Sent), "");
}

#[test]
fn backspace_pops_input_buffer() {
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('/')));
    for c in "abc".chars() {
        dispatch(&mut app, key(KeyCode::Char(c)));
    }
    assert_eq!(app.mailbox.search_text(app.mailbox_tab), "abc");
    dispatch(&mut app, key(KeyCode::Backspace));
    assert_eq!(app.mailbox.search_text(app.mailbox_tab), "ab");
}

#[test]
fn enter_confirms_keeps_typed_text() {
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('f')));
    for c in "kian".chars() {
        dispatch(&mut app, key(KeyCode::Char(c)));
    }
    dispatch(&mut app, key(KeyCode::Enter));
    assert!(
        app.mailbox_input_mode.is_none(),
        "input must close on Enter"
    );
    assert_eq!(
        app.mailbox.filter_text(app.mailbox_tab),
        "kian",
        "Enter must keep the typed text (confirm-keep semantics)"
    );
}

#[test]
fn esc_cancels_reverts_to_snapshot() {
    let mut app = app_with_mailbox_focused();
    // Seed a prior filter so the snapshot has something to restore.
    app.mailbox
        .set_input(app.mailbox_tab, MailboxInputKind::Filter, "previous".into());
    dispatch(&mut app, key(KeyCode::Char('f')));
    // Now overwrite via typing.
    dispatch(&mut app, key(KeyCode::Backspace));
    dispatch(&mut app, key(KeyCode::Backspace));
    dispatch(&mut app, key(KeyCode::Char('x')));
    assert_eq!(app.mailbox.filter_text(app.mailbox_tab), "previox");
    // Esc → revert.
    dispatch(&mut app, key(KeyCode::Esc));
    assert!(app.mailbox_input_mode.is_none());
    assert_eq!(
        app.mailbox.filter_text(app.mailbox_tab),
        "previous",
        "Esc must revert the active buffer to the pre-open snapshot"
    );
}

#[test]
fn open_input_swallows_pr1_cursor_keys() {
    // While input is open, Up/Down/j/k/PageUp/PageDown/Home/End
    // must NOT move the row cursor — they're swallowed by the
    // input-mode catchall arm.
    let mut app = app_with_mailbox_focused();
    // Seed buffers so the cursor has somewhere to move.
    app.mailbox.extend(
        app.mailbox_tab,
        (1..=10)
            .map(|i| crate::mailbox::MessageRow {
                id: i,
                sender: "p:a".into(),
                recipient: "p:dev".into(),
                text: "x".into(),
                sent_at: 0.0,
            })
            .collect(),
    );
    let seated = app.mailbox.cursor(app.mailbox_tab).selected_idx;
    assert_eq!(seated, 9, "extend seats cursor at tail (PR-1 contract)");
    // Open filter, then try to move the cursor — must not move.
    dispatch(&mut app, key(KeyCode::Char('f')));
    dispatch(&mut app, key(KeyCode::Up));
    dispatch(&mut app, key(KeyCode::PageUp));
    dispatch(&mut app, key(KeyCode::Home));
    // Cursor still at 9 (input ate `f` but those chars are part
    // of the filter buffer; Up/PageUp/Home are swallowed).
    // Note: typing `f` opens, but Up/PageUp/Home are not Char(_)
    // so they hit the catchall swallow arm.
    assert_eq!(app.mailbox.cursor(app.mailbox_tab).selected_idx, 9);
}

#[test]
fn ctrl_modifier_char_does_not_inject_into_input() {
    // qa #335 nit 2: Ctrl+W / Ctrl+C / Alt+Char while filter is
    // open must NOT land their literal char in the buffer.
    // Modifier combos fall through the Char-arm's guard and hit
    // the swallow arm — operator can still type plain `w` to
    // search for that char.
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('f'))); // open filter
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    dispatch(
        &mut app,
        key_with(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    dispatch(&mut app, key_with(KeyCode::Char('a'), KeyModifiers::ALT));
    assert_eq!(
        app.mailbox.filter_text(app.mailbox_tab),
        "",
        "modifier+Char combos must not leak into the filter buffer"
    );
    // Plain Char (no modifier) still types — sanity that the
    // guard didn't lock everyone out.
    dispatch(&mut app, key(KeyCode::Char('w')));
    assert_eq!(app.mailbox.filter_text(app.mailbox_tab), "w");
    // Shift+Char (capital letter shape) also types — Shift is
    // explicitly allowed in the guard.
    dispatch(&mut app, key_with(KeyCode::Char('X'), KeyModifiers::SHIFT));
    assert_eq!(app.mailbox.filter_text(app.mailbox_tab), "wX");
}

#[test]
fn open_input_swallows_q_quit() {
    // The killer test: pressing `q` while filter is open MUST go
    // into the filter buffer, NOT trigger the quit confirm.
    // (Char(c)-with-input-mode-guard arm placed BEFORE
    // `Char('q')` quit arm in match order.)
    let mut app = app_with_mailbox_focused();
    dispatch(&mut app, key(KeyCode::Char('f')));
    dispatch(&mut app, key(KeyCode::Char('q')));
    assert_eq!(
        app.stage,
        Stage::Triptych,
        "q must NOT trigger quit while input is open"
    );
    assert_eq!(
        app.mailbox.filter_text(app.mailbox_tab),
        "q",
        "q must land in the filter buffer"
    );
}

// T-131 PR-3: mailbox detail modal — open/close/scroll +
// snapshot-at-open contract.

fn seed_inbox_rows(app: &mut App, n: i64) {
    let rows: Vec<MessageRow> = (1..=n)
        .map(|i| MessageRow {
            id: i,
            sender: "p:dev".into(),
            recipient: "p:mgr".into(),
            text: format!("body #{i}"),
            sent_at: 1_700_000_000.0 + i as f64,
        })
        .collect();
    app.mailbox.extend(MailboxTab::Inbox, rows);
}

#[test]
fn enter_on_mailbox_opens_detail_modal_with_snapshot() {
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 5);
    // Cursor seats at tail (row id 5) per PR-1 contract.
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.stage, Stage::MailboxDetailModal);
    let snap = app.mailbox_detail_modal.as_ref().expect("modal open");
    assert_eq!(snap.id, 5);
    assert_eq!(snap.text, "body #5");
    assert_eq!(app.mailbox_detail_scroll, 0, "scroll resets on open");
}

#[test]
fn enter_on_empty_visible_indices_is_noop() {
    // Filter to nothing → Enter must NOT flip stage or snapshot.
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 3);
    app.mailbox.set_input(
        MailboxTab::Inbox,
        MailboxInputKind::Filter,
        "no-such-sender".into(),
    );
    assert!(app.mailbox.visible_indices(MailboxTab::Inbox).is_empty());
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.stage, Stage::Triptych);
    assert!(app.mailbox_detail_modal.is_none());
}

#[test]
fn snapshot_stable_across_underlying_drain() {
    // The variant-(a) killer test: open modal on row id 3, then
    // drain the buffer past it. Modal still renders id 3 because
    // the snapshot owns the content, not the underlying buffer.
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 5);
    app.mailbox.cursor_home(MailboxTab::Inbox);
    app.mailbox.move_cursor_down(MailboxTab::Inbox);
    app.mailbox.move_cursor_down(MailboxTab::Inbox); // selected_idx = 2 → row id 3
    dispatch(&mut app, key(KeyCode::Enter));
    let snap_id = app.mailbox_detail_modal.as_ref().expect("open").id;
    assert_eq!(snap_id, 3);
    // Now drain the front by pushing enough rows to trim past
    // row id 3. MAX_TAB_ROWS = 500.
    let more: Vec<MessageRow> = (6..=600)
        .map(|i| MessageRow {
            id: i,
            sender: "p:dev".into(),
            recipient: "p:mgr".into(),
            text: format!("body #{i}"),
            sent_at: 1_700_000_000.0 + i as f64,
        })
        .collect();
    app.mailbox.extend(MailboxTab::Inbox, more);
    // The original row id 3 should no longer be in the buffer.
    let still_there = app
        .mailbox
        .rows(MailboxTab::Inbox)
        .iter()
        .any(|r| r.id == 3);
    assert!(!still_there, "row id 3 must have been drained");
    // But the modal snapshot is unchanged — operator sees the
    // message they clicked, full stop.
    let snap = app.mailbox_detail_modal.as_ref().expect("still open");
    assert_eq!(snap.id, 3, "snapshot id must survive underlying drain");
    assert_eq!(snap.text, "body #3");
}

#[test]
fn esc_closes_detail_modal() {
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 3);
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.stage, Stage::MailboxDetailModal);
    dispatch(&mut app, key(KeyCode::Esc));
    assert_eq!(app.stage, Stage::Triptych);
    assert!(app.mailbox_detail_modal.is_none());
}

#[test]
fn q_closes_detail_modal() {
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 3);
    dispatch(&mut app, key(KeyCode::Enter));
    dispatch(&mut app, key(KeyCode::Char('q')));
    assert_eq!(app.stage, Stage::Triptych);
    assert!(app.mailbox_detail_modal.is_none());
}

#[test]
fn j_and_k_scroll_body_in_modal() {
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 3);
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.mailbox_detail_scroll, 0);
    dispatch(&mut app, key(KeyCode::Char('j')));
    dispatch(&mut app, key(KeyCode::Char('j')));
    dispatch(&mut app, key(KeyCode::Down));
    assert_eq!(app.mailbox_detail_scroll, 3);
    dispatch(&mut app, key(KeyCode::Char('k')));
    dispatch(&mut app, key(KeyCode::Up));
    assert_eq!(app.mailbox_detail_scroll, 1);
    // Saturating: more `k`s than current offset clamp at 0.
    for _ in 0..10 {
        dispatch(&mut app, key(KeyCode::Char('k')));
    }
    assert_eq!(app.mailbox_detail_scroll, 0);
}

#[test]
fn unrelated_keys_swallowed_in_modal() {
    // While modal open, `f` / `/` / `Tab` must not trigger their
    // Triptych meanings — the modal owns the stage.
    let mut app = app_with_mailbox_focused();
    seed_inbox_rows(&mut app, 3);
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.stage, Stage::MailboxDetailModal);
    let focused_before = app.focused_pane;
    dispatch(&mut app, key(KeyCode::Char('f')));
    dispatch(&mut app, key(KeyCode::Char('/')));
    dispatch(&mut app, key(KeyCode::Tab));
    assert_eq!(app.stage, Stage::MailboxDetailModal, "stage stays");
    assert!(app.mailbox_input_mode.is_none(), "filter/search not opened");
    assert_eq!(
        app.focused_pane, focused_before,
        "Tab must not cycle panes underneath an open modal"
    );
}
