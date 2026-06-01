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
    // T-131 PR-4: pin the timezone so the mailbox-row absolute-time
    // indicator (chrono::Local in production) is deterministic
    // across CI / dev machines. With sent_at=0.0 + now_secs=0.0 in
    // fixtures, this renders `00:00` (same-day fold).
    std::env::set_var("TZ", "UTC");
    App::new()
}

#[test]
fn splash_layout_at_120x30() {
    let app = fresh_app();
    assert_eq!(app.stage, Stage::Splash);
    let buf = render_to_buffer(&app, 120, 30);
    // The splash line embeds the crate version (`v0.8.7`), so a version bump
    // would otherwise re-break this snapshot and turn `cargo test --workspace`
    // red on every release. Redact the version token to a placeholder before
    // the assert so bumps never touch this snapshot again. Scoped to this test;
    // layout/structure is still pinned (only the version token is normalized).
    // (#377)
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"v\d+\.\d+\.\d+", "v[VERSION]");
    settings.bind(|| {
        insta::assert_snapshot!("splash_120x30", buffer_to_string(&buf));
    });
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
    app.cycle_focus(); // Agents → Detail
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
    //
    // T-209: the layout grew a third row (bottom status bar with cwd
    // + CPU/RAM), so the statusline is now the SECOND-to-last line,
    // not the last. The last line is the new status bar.
    let mut app = fresh_app();
    app.dismiss_splash();
    let buf = render_to_buffer(&app, 80, 10);
    let s = buffer_to_string(&buf);
    let lines: Vec<&str> = s.lines().collect();
    let statusline = lines
        .get(lines.len().saturating_sub(2))
        .copied()
        .expect("buffer not empty");
    assert!(
        statusline.contains("t tutorial"),
        "statusline missing tutorial hint at 80 cols: {statusline:?}"
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
        display_name: None,
        rate_limit_resets_at: None,
        reports_to: None,
    }
}

/// T-211: build a worker agent that reports to a manager by short
/// name (the YAML key, e.g. `mgr` not `p:mgr`). Used by the
/// reports_to-tree fixture below.
fn synth_worker_reporting_to(id: &str, manager_short_name: &str, state: AgentState) -> AgentInfo {
    let mut info = synth_agent(id, state, 0, 0);
    info.reports_to = Some(manager_short_name.into());
    info
}

fn fixture_team(team_name: &str, agents: Vec<AgentInfo>) -> TeamSnapshot {
    TeamSnapshot {
        root: std::path::PathBuf::from("/fixture"),
        team_name: team_name.into(),
        agents,
        channels: Vec::new(),
    }
}

#[test]
fn agents_panel_renders_glyphs_at_120x30() {
    // The Agents sidebar pulls from `app.team.agents` with state-
    // glyph mapping. Pin one of each glyph: running, working/unread,
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
    insta::assert_snapshot!("agents_panel_with_glyphs_120x30", buffer_to_string(&buf));
}

#[test]
fn agents_panel_renders_reports_to_tree_at_120x30() {
    // T-211: when at least one agent has `reports_to` set, the Agents
    // sidebar nests reports under their manager with `├─` / `└─`
    // tree glyphs. This fixture team has one manager + three workers
    // reporting to it, plus a second manager standalone — the
    // produced render shows the workers nested under their parent,
    // and the lone manager at depth 0 with no tree glyph.
    //
    // Fixture is passed through `into_tree_dfs_order` to mirror
    // production's `TeamSnapshot::load` reordering. Test input order
    // (managers first, then workers) is the same shape `load()`
    // produces post-sort.
    use teamctl_ui::data::into_tree_dfs_order;
    let mut app = fresh_app();
    app.dismiss_splash();
    let mut mgr_a = synth_agent("writing:mgr-a", AgentState::Running, 0, 0);
    mgr_a.is_manager = true;
    let mut mgr_b = synth_agent("writing:mgr-b", AgentState::Running, 0, 0);
    mgr_b.is_manager = true;
    let agents = into_tree_dfs_order(vec![
        mgr_a,
        mgr_b,
        synth_worker_reporting_to("writing:scribe", "mgr-a", AgentState::Running),
        synth_worker_reporting_to("writing:critic", "mgr-a", AgentState::Stopped),
        synth_worker_reporting_to("writing:scout", "mgr-a", AgentState::Unknown),
    ]);
    app.replace_team(fixture_team("writing-team", agents));
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!(
        "agents_panel_with_reports_to_tree_120x30",
        buffer_to_string(&buf)
    );
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
    // Cycle focus to Mailbox (Agents → Detail → Mailbox).
    app.cycle_focus();
    app.cycle_focus();
    assert_eq!(app.focused_pane, Pane::Mailbox);
    // Tab on Mailbox cycles tabs. Two cycles to reach Channel
    // (Inbox → Sent → Channel).
    app.cycle_mailbox_tab();
    app.cycle_mailbox_tab();
    assert_eq!(app.mailbox_tab, teamctl_ui::mailbox::MailboxTab::Channel);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_channel_focused_120x30", buffer_to_string(&buf));
}

fn approval(id: i64, action: &str, summary: &str) -> teamctl_ui::approvals::Approval {
    teamctl_ui::approvals::Approval {
        id,
        project_id: "writing".into(),
        agent_id: "writing:manager".into(),
        action: action.into(),
        summary: summary.into(),
        payload_json: String::new(),
    }
}

#[test]
fn approvals_stripe_renders_when_pending() {
    // PR-UI-4: the conditional stripe at the top of Triptych
    // appears only when `pending_approvals` is non-empty.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.replace_approvals(vec![
        approval(7, "publish", "post the morning brief"),
        approval(8, "deploy", "ship docs"),
    ]);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let first_line = s.lines().next().expect("non-empty buffer");
    assert!(
        first_line.contains("approvals: 2 pending") && first_line.contains("`a` to review"),
        "stripe missing or malformed: {first_line:?}"
    );
    insta::assert_snapshot!("approvals_stripe_120x30", s);
}

#[test]
fn approvals_modal_renders_action_summary_and_hint() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.replace_approvals(vec![approval(
        7,
        "publish",
        "Post the morning brief to r/yourcity",
    )]);
    app.enter_approvals_modal();
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("approvals · 1/1"), "modal title missing");
    assert!(s.contains("publish"), "action missing");
    assert!(s.contains("[y] approve"), "action hint missing");
    assert!(
        s.contains("[Shift-N] deny"),
        "deny hint must signal Shift-gate"
    );
    insta::assert_snapshot!("approvals_modal_120x30", s);
}

#[test]
fn approvals_modal_multi_row_cursor_advanced_at_120x30() {
    // T-079-D snapshot: 3-row queue with the cursor advanced once
    // (operator hit `j`). Pins the title counter (`2/3`) and that
    // the focused row's summary is the second one — the rendering
    // proof that navigation reached the right row.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.replace_approvals(vec![
        approval(7, "publish", "Post the morning brief to r/yourcity"),
        approval(8, "deploy", "Ship docs site to production"),
        approval(9, "merge", "Land PR #123 to main"),
    ]);
    app.enter_approvals_modal();
    app.cycle_approval_next();
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        s.contains("approvals · 2/3"),
        "modal title must show 2/3 after one j: {s}"
    );
    assert!(
        s.contains("Ship docs site to production"),
        "focused row summary must be the second row: {s}"
    );
    insta::assert_snapshot!("approvals_modal_multi_row_at_2of3_120x30", s);
}

#[test]
fn compose_modal_renders_target_body_and_attach_footer() {
    // PR-UI-5 + T-32: compose modal opens with the DM target in
    // the title bar, the editor body in the middle, and a footer
    // advertising the `Tab attach` affordance (T-32 wired the
    // path-input overlay; before T-32 the footer carried a TODO).
    let mut app = fresh_app();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    app.dismiss_splash();
    app.select_next(); // focus dev1
    app.enter_compose_dm_for_focused();
    // Type two lines into the editor.
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let press = |code: KeyCode| KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    for c in "line one".chars() {
        app.compose_editor.apply_key(press(KeyCode::Char(c)));
    }
    app.compose_editor.apply_key(press(KeyCode::Enter));
    for c in "line two".chars() {
        app.compose_editor.apply_key(press(KeyCode::Char(c)));
    }
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("→ writing:dev1"), "title missing: {s}");
    assert!(
        s.contains("line one") && s.contains("line two"),
        "body missing"
    );
    assert!(s.contains("Tab attach"), "footer attach hint missing: {s}");
    assert!(
        !s.contains("TODO #32"),
        "footer should not carry the TODO once the affordance ships: {s}"
    );
    insta::assert_snapshot!("compose_modal_120x30", s);
}

// ── PR-UI-7 fixup (qa Gap D): detail-pane splits actually render.

#[test]
fn detail_pane_two_vertical_splits_renders_side_by_side() {
    use teamctl_ui::app::SplitOrientation;
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    app.set_detail_buffer(["[12:00] focused".into()].to_vec());
    // Inject splits directly to keep the snapshot input deterministic.
    app.detail_splits = vec![("writing:dev1".into(), SplitOrientation::Vertical)];
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("writing:manager"), "focused agent title missing");
    assert!(s.contains("writing:dev1"), "split agent title missing");
    insta::assert_snapshot!("detail_two_vertical_splits_120x30", s);
}

#[test]
fn detail_pane_two_horizontal_splits_stack_top_to_bottom() {
    use teamctl_ui::app::SplitOrientation;
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
        ],
    ));
    app.set_detail_buffer(["[12:00] focused".into()].to_vec());
    app.detail_splits = vec![("writing:dev1".into(), SplitOrientation::Horizontal)];
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("writing:manager"));
    assert!(s.contains("writing:dev1"));
    insta::assert_snapshot!("detail_two_horizontal_splits_120x30", s);
}

#[test]
fn detail_pane_four_split_mixed_grid_renders() {
    use teamctl_ui::app::SplitOrientation;
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:dev1", AgentState::Running, 0, 0),
            synth_agent("writing:dev2", AgentState::Running, 0, 0),
            synth_agent("writing:critic", AgentState::Running, 0, 0),
        ],
    ));
    app.set_detail_buffer(["[12:00] focused".into()].to_vec());
    // Composition: vertical → horizontal → vertical → horizontal.
    // First V starts column 1 next to focused; following H stacks
    // inside that new column; then another V opens a third
    // column; another H stacks inside it. Net: 3 columns, the
    // first holds the focused agent, the second holds dev1+dev2
    // stacked, the third holds critic alone.
    app.detail_splits = vec![
        ("writing:dev1".into(), SplitOrientation::Vertical),
        ("writing:dev2".into(), SplitOrientation::Horizontal),
        ("writing:critic".into(), SplitOrientation::Vertical),
    ];
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    for must in [
        "writing:manager",
        "writing:dev1",
        "writing:dev2",
        "writing:critic",
    ] {
        assert!(s.contains(must), "missing split title: {must}");
    }
    insta::assert_snapshot!("detail_four_split_mixed_120x30", s);
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

#[test]
fn wall_layout_renders_tile_grid_at_120x30() {
    // T-079-C snapshot: Wall layout (Ctrl+W) reached after the
    // operator flips from Triptych. Tiles render as a 2×2 grid
    // with the first four agents; the AGENTS/MAILBOX pane chrome
    // from Triptych is gone, replaced by per-tile borders carrying
    // the agent id + state glyph.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:worker-1", AgentState::Running, 0, 0),
            synth_agent("writing:worker-2", AgentState::Running, 0, 0),
            synth_agent("writing:critic", AgentState::Stopped, 0, 0),
        ],
    ));
    app.toggle_wall_layout();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("wall_layout_120x30", buffer_to_string(&buf));
}

fn agent_with_label(id: &str, display: &str, state: AgentState) -> AgentInfo {
    let mut info = synth_agent(id, state, 0, 0);
    info.display_name = Some(display.into());
    info
}

#[test]
fn roster_renders_display_name_when_set() {
    // T-160: the Agents sidebar (roster pane) renders `display_name`
    // in place of the raw YAML key for agents that have one set;
    // agents without a display_name keep the existing short-form
    // (`info.agent`) fallback.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            agent_with_label("writing:manager", "Manager (Lead)", AgentState::Running),
            synth_agent("writing:worker-1", AgentState::Running, 0, 0),
        ],
    ));
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        s.contains("Manager (Lead)"),
        "roster missing display_name `Manager (Lead)`: {s}"
    );
    assert!(
        s.contains("worker-1"),
        "roster missing short-form fallback for agent without display_name: {s}"
    );
}

#[test]
fn detail_header_renders_display_name_when_set() {
    // T-160: the Detail pane border title (`DETAIL · <label>`) swaps
    // the canonical id for `display_name` when the selected agent has
    // one set. Cross-project clarity is preserved by the fallback to
    // the canonical id when `display_name` is absent.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![agent_with_label(
            "writing:manager",
            "Manager (Lead)",
            AgentState::Running,
        )],
    ));
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("DETAIL"), "detail pane not rendered: {s}");
    assert!(
        s.contains("Manager (Lead)"),
        "detail title missing display_name `Manager (Lead)`: {s}"
    );
}

#[test]
fn wall_tile_title_renders_display_name_when_set() {
    // T-160: Wall layout tile titles (the per-tile borders that
    // replace the AGENTS/MAILBOX pane chrome) carry `display_name` in
    // place of the canonical id when set.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            agent_with_label("writing:manager", "Manager (Lead)", AgentState::Running),
            synth_agent("writing:worker-1", AgentState::Running, 0, 0),
            synth_agent("writing:worker-2", AgentState::Running, 0, 0),
            synth_agent("writing:critic", AgentState::Stopped, 0, 0),
        ],
    ));
    app.toggle_wall_layout();
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        s.contains("Manager (Lead)"),
        "wall tile title missing display_name `Manager (Lead)`: {s}"
    );
    assert!(
        s.contains("writing:worker-1"),
        "wall tile title missing canonical-id fallback for agent without display_name: {s}"
    );
}

#[test]
fn mailbox_first_layout_renders_channel_focused_at_120x30() {
    // T-079-C snapshot: MailboxFirst layout (Ctrl+M). Channel list
    // / feed / participants split replaces the Triptych panes;
    // first entry seeds the channel cursor per
    // `toggle_mailbox_first_layout`.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![
            synth_agent("writing:manager", AgentState::Running, 0, 0),
            synth_agent("writing:worker-1", AgentState::Running, 0, 0),
        ],
    ));
    app.toggle_mailbox_first_layout();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_first_layout_120x30", buffer_to_string(&buf));
}

#[test]
fn mailbox_pane_renders_channel_tab_with_rows() {
    // T-079-E coverage extension: the existing
    // `mailbox_pane_cycles_to_channel_tab_when_focused` pins the
    // empty Channel tab; this fixture pins the populated shape so a
    // future regression that fails to render channel rows shows up
    // as a glyph diff rather than a behavioural surprise in the
    // production TUI.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.cycle_focus();
    app.cycle_focus();
    app.cycle_mailbox_tab(); // Inbox → Sent
    app.cycle_mailbox_tab(); // Sent → Channel
    app.mailbox.extend(
        teamctl_ui::mailbox::MailboxTab::Channel,
        vec![
            message(
                21,
                "writing:dev1",
                "channel:writing:devs",
                "stack rebased on main",
            ),
            message(22, "writing:critic", "channel:writing:devs", "looks tight"),
        ],
    );
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_channel_with_rows_120x30", buffer_to_string(&buf));
}

#[test]
fn mailbox_pane_renders_wire_tab_with_rows() {
    // T-079-E coverage extension: the Wire tab carries
    // project-broadcast traffic (recipient `channel:<project>:all`).
    // Pinned here so regressions that confuse the Wire filter or
    // its rendering surface show up immediately.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.cycle_focus();
    app.cycle_focus();
    app.cycle_mailbox_tab(); // Inbox → Sent
    app.cycle_mailbox_tab(); // Sent → Channel
    app.cycle_mailbox_tab(); // Channel → Wire
    app.mailbox.extend(
        teamctl_ui::mailbox::MailboxTab::Wire,
        vec![
            message(
                31,
                "user:cli",
                "channel:writing:all",
                "0.7.1 release cut · CHANGELOG updated",
            ),
            message(
                32,
                "writing:eng_lead",
                "channel:writing:all",
                "T-079 cluster Wave 3 dispatching",
            ),
        ],
    );
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_wire_with_rows_120x30", buffer_to_string(&buf));
}

#[test]
fn mailbox_pane_renders_sent_tab_with_rows() {
    // T-122: the Sent tab carries every row whose sender is the
    // focused agent — DMs they sent, telegram replies, channel
    // posts, wire broadcasts. Pinned here so a future regression that
    // confuses the sender filter (or fails to render mixed-recipient
    // rows on one tab) shows up immediately.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.cycle_focus();
    app.cycle_focus();
    app.cycle_mailbox_tab(); // Inbox → Sent
    app.mailbox.extend(
        teamctl_ui::mailbox::MailboxTab::Sent,
        vec![
            message(
                41,
                "writing:manager",
                "writing:dev1",
                "T-079 needs the wave-3 split first",
            ),
            message(
                42,
                "writing:manager",
                "channel:writing:devs",
                "blocking on review for #88",
            ),
            message(
                43,
                "writing:manager",
                "user:telegram",
                "all 5 PRs merged · 0.7.1 ready",
            ),
        ],
    );
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("mailbox_sent_with_rows_120x30", buffer_to_string(&buf));
}

#[test]
fn stream_keys_mode_renders_banner_and_pane_marker() {
    // T-108: when stream-mode is active the operator must see two
    // unambiguous indicators — the full-width statusline banner and
    // the `[STREAM-KEYS]` tag in the detail pane title. Pin both so
    // a future statusline / triptych refactor can't silently drop
    // the affordance the operator relies on to know "I'm typing
    // into the agent right now."
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.cycle_focus(); // Roster → Detail
    app.enter_stream_keys();
    assert_eq!(app.stage, Stage::StreamKeys);
    let buf = render_to_buffer(&app, 120, 30);
    let rendered = buffer_to_string(&buf);
    assert!(
        rendered.contains("STREAM-KEYS → writing:manager"),
        "statusline banner missing the target id"
    );
    assert!(
        rendered.contains("Ctrl+E to exit"),
        "statusline banner missing the exit hint"
    );
    assert!(
        rendered.contains("[STREAM-KEYS]"),
        "detail pane title missing the stream-mode tag"
    );
    insta::assert_snapshot!("stream_keys_banner_120x30", rendered);
}

// ── T-209: bottom status bar (cwd left + CPU/RAM right) ──────────

#[test]
fn status_bar_renders_path_left_and_metrics_right_at_120x30() {
    // Plant a deterministic team root on the App so the path slot
    // has known content (the default empty PathBuf would render as
    // empty string, which is a degenerate case). sysinfo's untouched
    // numbers stay at zero — that's the intended uninit shape for
    // snapshots (a refresh tick fires at runtime, not in tests).
    let mut app = fresh_app();
    app.dismiss_splash();
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains("/tmp/teamctl-fixture/.team"),
        "status bar missing team-root path at 120 cols: {last_line:?}"
    );
    assert!(
        last_line.contains("CPU "),
        "status bar missing CPU label: {last_line:?}"
    );
    assert!(
        last_line.contains("RAM "),
        "status bar missing RAM label: {last_line:?}"
    );
}

#[test]
fn status_bar_truncates_path_when_narrow_keeps_metrics_visible() {
    // At a comfortably wide-enough width, both slots fit and the
    // path renders with a head-and-tail ellipsis. The basename
    // (`.team` here) MUST stay visible — losing it would make the
    // status bar useless on multi-project hosts.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.team.root = std::path::PathBuf::from(
        "/home/operator/very/long/nested/project/path/teamctl-deep-nest/.team",
    );
    let buf = render_to_buffer(&app, 100, 20);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains(".team"),
        "narrow status bar dropped the basename: {last_line:?}"
    );
    assert!(
        last_line.contains("CPU "),
        "narrow status bar dropped CPU label: {last_line:?}"
    );
}

#[test]
fn status_bar_elides_metrics_when_too_narrow_for_both_slots() {
    // 30 cols: too narrow for both path AND CPU/RAM. Path wins —
    // operator's "WHERE am I" matters more than live metrics.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let buf = render_to_buffer(&app, 30, 20);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains(".team"),
        "narrow status bar dropped the basename: {last_line:?}"
    );
    assert!(
        !last_line.contains("CPU "),
        "expected CPU to elide on 30-col width: {last_line:?}"
    );
}

// ── T-212: per-agent rate-limit indicator (center slot) ──────────

fn unix_now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[test]
fn status_bar_renders_rate_limit_for_focused_agent_with_active_window() {
    // Focused agent has a rate-limit window ~1h in the future.
    // Status bar should render a `limit Xh Ym` token between the
    // path (left) and the CPU/RAM block (right).
    //
    // Preview gate (T-212): tests flip `rate_limit_indicator_enabled`
    // directly on the App rather than racing on a process-wide env
    // var. The production path reads the env var once at App::new().
    let mut app = fresh_app();
    app.dismiss_splash();
    app.rate_limit_indicator_enabled = true;
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let mut agent = synth_agent("p:a", AgentState::Running, 0, 0);
    agent.rate_limit_resets_at = Some(unix_now_secs() + 3661.0); // 1h 1m 1s out
    app.replace_team(fixture_team("test", vec![agent]));
    app.selected_agent = Some(0);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains("limit "),
        "status bar missing rate-limit indicator: {last_line:?}"
    );
}

#[test]
fn status_bar_omits_rate_limit_when_preview_flag_disabled() {
    // Active rate-limit window present, but the preview flag is OFF
    // (default). The center slot stays blank — this is the shape
    // operators see by default until they opt in via
    // `TEAMCTL_UI_RATE_LIMIT_INDICATOR=1`.
    let mut app = fresh_app();
    app.dismiss_splash();
    // Default: rate_limit_indicator_enabled = false (env var unset).
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let mut agent = synth_agent("p:a", AgentState::Running, 0, 0);
    agent.rate_limit_resets_at = Some(unix_now_secs() + 3661.0);
    app.replace_team(fixture_team("test", vec![agent]));
    app.selected_agent = Some(0);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        !last_line.contains("limit "),
        "preview-gated indicator rendered with flag off: {last_line:?}"
    );
}

#[test]
fn status_bar_omits_rate_limit_when_focused_agent_has_no_window() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.rate_limit_indicator_enabled = true;
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    // synth_agent defaults rate_limit_resets_at to None.
    let agent = synth_agent("p:a", AgentState::Running, 0, 0);
    app.replace_team(fixture_team("test", vec![agent]));
    app.selected_agent = Some(0);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        !last_line.contains("limit "),
        "status bar rendered indicator with no active window: {last_line:?}"
    );
}

#[test]
fn status_bar_omits_rate_limit_when_focused_agent_window_is_in_the_past() {
    // A `resets_at` from before now is treated the same as None —
    // the limit has already expired, the indicator hides.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.rate_limit_indicator_enabled = true;
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let mut agent = synth_agent("p:a", AgentState::Running, 0, 0);
    agent.rate_limit_resets_at = Some(1.0);
    app.replace_team(fixture_team("test", vec![agent]));
    app.selected_agent = Some(0);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        !last_line.contains("limit "),
        "status bar rendered indicator for expired window: {last_line:?}"
    );
}

#[test]
fn status_bar_swaps_rate_limit_with_focused_agent() {
    // Two agents — one with an active window, one without. The
    // indicator should appear when the limited agent is focused
    // and disappear when focus moves to the unlimited one.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.rate_limit_indicator_enabled = true;
    app.team.root = std::path::PathBuf::from("/tmp/teamctl-fixture/.team");
    let mut limited = synth_agent("p:limited", AgentState::Running, 0, 0);
    limited.rate_limit_resets_at = Some(unix_now_secs() + 600.0); // 10m
    let calm = synth_agent("p:calm", AgentState::Running, 0, 0);
    app.replace_team(fixture_team("test", vec![limited, calm]));

    app.selected_agent = Some(0);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        s.lines()
            .last()
            .map(|l| l.contains("limit "))
            .unwrap_or(false),
        "indicator missing when limited agent focused"
    );

    app.selected_agent = Some(1);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        !s.lines()
            .last()
            .map(|l| l.contains("limit "))
            .unwrap_or(true),
        "indicator persisted when focus moved to unlimited agent"
    );
}

#[test]
fn status_bar_omits_rate_limit_indicator_when_path_crowds_center_slot() {
    // Truncation contract per coordination with otis on T-209:
    // path > per-agent > metrics. Metrics elide first when the bar
    // is too narrow; the indicator drops only when there's no room
    // between the path's rendered right edge and the metrics x
    // (or, when metrics have elided, the area's right edge).
    //
    // Set up a width where path + metrics fit comfortably but the
    // path is wide enough to leave no center slot. The indicator
    // must elide while path AND metrics both stay visible.
    let mut app = fresh_app();
    app.dismiss_splash();
    app.rate_limit_indicator_enabled = true;
    app.team.root = std::path::PathBuf::from("/operator/dev/projects/teamctl-deep-nest/.team");
    let mut agent = synth_agent("p:a", AgentState::Running, 0, 0);
    agent.rate_limit_resets_at = Some(unix_now_secs() + 600.0);
    let mut team = fixture_team("test", vec![agent]);
    team.root = app.team.root.clone();
    app.replace_team(team);
    app.selected_agent = Some(0);
    // Path ~46 chars + metrics ~22 chars + 2 gutters = ~70. At 80
    // cols, free space is ~10 — too tight for `limit 10m 0s` (12).
    let buf = render_to_buffer(&app, 80, 20);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains(".team"),
        "status bar dropped the basename: {last_line:?}"
    );
    assert!(
        last_line.contains("CPU "),
        "status bar dropped CPU label: {last_line:?}"
    );
    assert!(
        !last_line.contains("limit "),
        "status bar should drop indicator before metrics when path crowds center: {last_line:?}"
    );
}

#[test]
fn mailbox_pane_renders_head_anchored_window_when_cursor_at_head() {
    // T-131 PR-1: with more rows than fit and the cursor jumped to
    // the head (`Home` / `g`-equivalent), the rendered window shows
    // the first rows, NOT the pre-T-131 tail. This is the snapshot
    // value-add — pins the cursor-aware window slicing distinct from
    // the pre-T-131 tail-only behavior the other mailbox snapshots
    // still pin for the default cursor-at-tail case.
    use teamctl_ui::mailbox::MailboxTab;
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    // 30 distinguishable rows — well over the mailbox pane height at
    // 120x30 (the right-stack mailbox slice is roughly the lower
    // 2/5 of 30 ≈ 12 rows). With cursor at tail (default), only the
    // last ~12 rows are visible. With cursor at head, the first ~12
    // are visible. The body text is the disambiguator.
    let batch: Vec<_> = (1..=30)
        .map(|i| {
            message(
                i,
                "writing:dev1",
                "writing:manager",
                &format!("msg #{i:02}"),
            )
        })
        .collect();
    app.mailbox.extend(MailboxTab::Inbox, batch);
    app.mailbox_cursor_home(); // selected_idx = 0, anchors window at head.
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    // The first row body (#01) must appear; the last row body (#30)
    // must NOT (it's below the visible window).
    assert!(
        s.contains("msg #01"),
        "head-anchored mailbox should render the first row;\nbuf:\n{s}"
    );
    assert!(
        !s.contains("msg #30"),
        "head-anchored mailbox must NOT render the last row;\nbuf:\n{s}"
    );
    insta::assert_snapshot!("mailbox_head_anchored_120x30", s);
}

#[test]
fn mailbox_pane_shows_filter_indicator_when_filter_set_and_input_closed() {
    // T-131 PR-2: with a non-empty filter and the input CLOSED, the
    // mailbox pane reserves one line between tabs and body for an
    // always-visible state indicator (`filter: foo`) — without it,
    // closing the input leaves a shorter row list with no signal why
    // rows disappeared (the UX bug the variant Q called out).
    // Additionally pins that the filter actually restricts the
    // visible rows: only senders matching the filter substring
    // appear in the body.
    use teamctl_ui::mailbox::{MailboxInputKind, MailboxTab};
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.mailbox.extend(
        MailboxTab::Inbox,
        vec![
            message(11, "writing:ada", "writing:manager", "ready for review"),
            message(12, "writing:kian", "writing:manager", "release notes"),
            message(13, "writing:ada", "writing:manager", "shipping the patch"),
            message(14, "user:telegram", "writing:manager", "any blockers?"),
        ],
    );
    app.mailbox
        .set_input(MailboxTab::Inbox, MailboxInputKind::Filter, "ada".into());
    assert!(app.mailbox_input_mode.is_none());
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(
        s.contains("filter: ada"),
        "filter-state indicator must be visible when filter set + input closed;\nbuf:\n{s}"
    );
    assert!(
        s.contains("ready for review") && s.contains("shipping the patch"),
        "ada's rows must remain visible;\nbuf:\n{s}"
    );
    assert!(
        !s.contains("release notes") && !s.contains("any blockers"),
        "non-ada rows must be hidden by the filter;\nbuf:\n{s}"
    );
    insta::assert_snapshot!("mailbox_filter_indicator_120x30", s);
}

#[test]
fn mailbox_detail_modal_renders_metadata_and_body() {
    // T-131 PR-3: open the detail modal on a selected row and pin
    // its rendered shape — metadata header (from / to / kind / time /
    // transport) + the wrapped body. Title carries the message id.
    use teamctl_ui::app::Stage;
    use teamctl_ui::mailbox::MailboxTab;
    let mut app = fresh_app();
    app.dismiss_splash();
    app.replace_team(fixture_team(
        "writing-team",
        vec![synth_agent("writing:manager", AgentState::Running, 0, 0)],
    ));
    app.mailbox.extend(
        MailboxTab::Inbox,
        vec![message(
            42,
            "user:telegram",
            "writing:manager",
            "shipping the detail modal — please review when you have a moment.",
        )],
    );
    // Cycle focus to Mailbox (Roster → Detail → Mailbox).
    app.cycle_focus();
    app.cycle_focus();
    app.open_mailbox_detail_modal();
    assert_eq!(app.stage, Stage::MailboxDetailModal);
    let buf = render_to_buffer(&app, 120, 30);
    let s = buffer_to_string(&buf);
    assert!(s.contains("MESSAGE"), "modal title missing:\n{s}");
    assert!(s.contains("id 42"), "message id missing:\n{s}");
    assert!(
        s.contains("via telegram"),
        "transport heuristic missing:\n{s}"
    );
    assert!(s.contains("DM"), "kind label missing:\n{s}");
    assert!(
        s.contains("shipping the detail modal"),
        "body missing:\n{s}"
    );
    insta::assert_snapshot!("mailbox_detail_modal_120x30", s);
}
