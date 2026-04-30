//! Golden-snapshot tests for the PR-UI-1 layout. Each test pins the
//! visible glyphs at a specific terminal size; insta diffs the
//! committed `*.snap` against the rendered buffer. Update with
//! `cargo insta review` when intentional layout changes land.
//!
//! Snapshots are intentionally rendered in monochrome (NO_COLOR=1
//! before `App::new`) so style sequences don't pollute the diff —
//! glyph layout is what we're pinning, not colour fidelity.

use ratatui::buffer::Buffer;
use team_core::supervisor::AgentState;
use teamctl_ui::app::{render_to_buffer, App, Stage};
use teamctl_ui::data::{AgentInfo, TeamSnapshot};
use teamctl_ui::triptych::Pane;

fn buffer_to_string(buf: &Buffer) -> String {
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

fn fresh_app() -> App {
    // Force monochrome so snapshots don't capture ANSI colour state.
    std::env::set_var("NO_COLOR", "1");
    App::new()
}

#[test]
fn splash_layout_at_120x30() {
    let app = fresh_app();
    assert_eq!(app.stage, Stage::Splash);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("splash_120x30", buffer_to_string(&buf));
}

#[test]
fn triptych_empty_state_at_120x30() {
    let mut app = fresh_app();
    app.dismiss_splash();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("triptych_empty_120x30", buffer_to_string(&buf));
}

#[test]
fn triptych_focus_ring_follows_focused_pane() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.cycle_focus(); // Roster → Detail
    assert_eq!(app.focused_pane, Pane::Detail);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("triptych_detail_focused_120x30", buffer_to_string(&buf));
}

#[test]
fn quit_confirm_overlay_at_120x30() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.enter_quit_confirm();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("quit_confirm_120x30", buffer_to_string(&buf));
}

#[test]
fn statusline_renders_tutorial_hint_at_right() {
    // The `· t tutorial` hint is always visible (SPEC §4); pin it
    // here at a narrow width to catch regressions where it gets
    // pushed off-screen by a wider left-side hint.
    let mut app = fresh_app();
    app.dismiss_splash();
    let buf = render_to_buffer(&app, 80, 10);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains("t tutorial"),
        "statusline missing tutorial hint at 80 cols: {last_line:?}"
    );
}

fn synth_agent(id: &str, state: AgentState, unread: u32, pending: u32) -> AgentInfo {
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

fn fixture_team(team_name: &str, agents: Vec<AgentInfo>) -> TeamSnapshot {
    TeamSnapshot {
        root: std::path::PathBuf::from("/fixture"),
        team_name: team_name.into(),
        agents,
    }
}

#[test]
fn roster_renders_agents_with_glyphs_at_120x30() {
    // PR-UI-2: roster pulls from `app.team.agents` with state-glyph
    // mapping. Pin one of each glyph: running, working/unread,
    // pending-approval, stopped, unknown.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:worker-1", AgentState::Running, 3, 0),
            synth_agent("writing:worker-2", AgentState::Running, 0, 1),
            synth_agent("writing:critic", AgentState::Stopped, 0, 0),
            synth_agent("writing:scout", AgentState::Unknown, 0, 0),
        ],
    ));
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("roster_with_agents_120x30", buffer_to_string(&buf));
}

#[test]
fn detail_pane_streams_buffer_for_selected_agent() {
    // With an agent selected and a non-empty detail_buffer the
    // detail pane should show the buffer's tail; the title carries
    // the focused agent id so the operator knows which session.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:worker-1", AgentState::Running, 0, 0),
        ],
    ));
    app.set_detail_buffer(
        [
            "[12:00] user: draft a release plan",
            "[12:01] assistant: Sure — I'll outline the cascade.",
            "[12:01] tool: teamctl validate",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("detail_streams_120x30", buffer_to_string(&buf));
}

fn message(id: i64, sender: &str, recipient: &str, text: &str) -> teamctl_ui::mailbox::MessageRow {
    teamctl_ui::mailbox::MessageRow {
        id,
        sender: sender.into(),
        recipient: recipient.into(),
        text: text.into(),
        sent_at: 0.0,
    }
}

#[test]
fn mailbox_pane_renders_inbox_tab_with_rows() {
    // PR-UI-3: mailbox pane shows the active tab's buffer rows.
    // Inbox is the default tab; the active-tab indicator gets the
    // REVERSED highlight (visible even in monochrome).
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.mailbox.extend(
        teamctl_ui::mailbox::MailboxTab::Inbox,
        vec![
            message(11, "writing:dev1", "writing:manager", "ready for review"),
            message(12, "user:telegram", "writing:manager", "any blockers?"),
        ],
    );
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_inbox_120x30", buffer_to_string(&buf));
}

#[test]
fn mailbox_pane_cycles_to_channel_tab_when_focused() {
    // Tab from the mailbox pane should advance the active tab; the
    // pane itself stays focused. Channel tab's empty hint shows
    // when the channel buffer has nothing yet.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    // Cycle focus to Mailbox (Roster → Detail → Mailbox).
    app.cycle_focus();
    app.cycle_focus();
    assert_eq!(app.focused_pane, Pane::Mailbox);
    // Tab on Mailbox cycles tabs.
    app.cycle_mailbox_tab();
    assert_eq!(app.mailbox_tab, teamctl_ui::mailbox::MailboxTab::Channel);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_channel_focused_120x30", buffer_to_string(&buf));
}

#[test]
fn render_at_minimum_terminal_does_not_panic() {
    // Small terminal — ratatui swallows over-large constraints, so as
    // long as the call doesn't panic we're good. (Smaller than ~16 wide
    // is degenerate; this pins the floor we care about.)
    let mut app = fresh_app();
    app.dismiss_splash();
    let _ = render_to_buffer(&app, 20, 8);
}
