use super::*;

#[test]
fn approval_refresh_preserves_selected_identity_when_earlier_rows_disappear() {
    let mut app = App::new();
    app.replace_approvals(vec![ap(1), ap(2), ap(3)]);
    app.enter_approvals_modal();
    app.selected_approval = 1;
    app.replace_approvals(vec![ap(2), ap(3), ap(4)]);
    assert_eq!(app.focused_approval().unwrap().id, 2);
    let decider = crate::approvals::test_support::MockApprovalDecider::default();
    app.apply_decision(&decider, Decision::Approve, "");
    assert_eq!(decider.calls.lock().unwrap()[0].0, 2);
}

#[test]
fn failed_approval_refresh_keeps_queue_and_modal() {
    struct Failing;
    impl ApprovalSource for Failing {
        fn pending(&self) -> Result<Vec<Approval>> {
            anyhow::bail!("database is locked")
        }
    }
    let mut app = App::new();
    app.replace_approvals(vec![ap(7)]);
    app.enter_approvals_modal();
    refresh_approvals(&mut app, &Failing);
    assert_eq!(app.stage, Stage::ApprovalsModal);
    assert_eq!(app.focused_approval().unwrap().id, 7);
    assert!(app.approval_error.as_deref().unwrap().contains("locked"));
}

#[test]
fn unrelated_key_consumes_window_chord_before_later_q() {
    let mut app = pane_sync_fixture();
    app.add_detail_split();
    dispatch(
        &mut app,
        key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    dispatch(&mut app, key(KeyCode::Char('x')));
    assert!(app.pending_chord.is_none());
    dispatch(&mut app, key(KeyCode::Char('q')));
    assert_eq!(app.stage, Stage::QuitConfirm);
    assert_eq!(app.detail_splits.len(), 1);
}

#[test]
fn all_n_plus_one_cells_are_reachable_and_close_by_visible_identity() {
    let mut app = pane_sync_fixture();
    app.add_detail_split(); // extra manager cell
    app.select_next();
    app.add_detail_split_horizontal(); // extra developer cell
    assert_eq!(app.selected_split, 2);
    assert_eq!(app.stream_target_session().as_deref(), Some("t-hello-dev"));
    app.cycle_split_next();
    assert_eq!(app.selected_split, 0);
    app.cycle_split_prev();
    assert_eq!(app.selected_split, 2);
    app.close_focused_split();
    assert_eq!(
        app.detail_splits,
        vec![("hello:mgr".into(), SplitOrientation::Vertical)]
    );
    assert_eq!(app.selected_split, 1);
    assert_eq!(app.stream_target_session().as_deref(), Some("t-hello-mgr"));
    app.close_other_splits();
    assert_eq!(app.selected_agent_id().as_deref(), Some("hello:mgr"));
    assert!(app.detail_splits.is_empty());
}

#[test]
fn closing_base_cell_promotes_the_first_extra_cell() {
    let mut app = pane_sync_fixture();
    app.add_detail_split();
    app.select_next();
    app.selected_split = 0;
    app.close_focused_split();
    assert_eq!(app.selected_agent_id().as_deref(), Some("hello:mgr"));
    assert!(app.detail_splits.is_empty());
    assert_eq!(app.selected_split, 0);
}

#[test]
fn capture_resize_and_stream_target_the_same_cell_with_rendered_geometry() {
    let mut app = pane_sync_fixture();
    app.detail_splits = vec![("hello:dev".into(), SplitOrientation::Vertical)];
    app.selected_split = 1;
    app.focused_pane = Pane::Detail;
    let source = crate::pane::test_support::MockPaneSource {
        lines: vec!["developer output".into()],
        ..Default::default()
    };
    recapture_focused_pane(&mut app, &source);
    assert_eq!(source.asked.lock().unwrap().as_slice(), ["t-hello-dev"]);
    let resizer = crate::pane_resize::test_support::MockPaneResizer::default();
    let total = Rect::new(0, 0, 120, 40);
    sync_focused_pane_size_to(&mut app, total, &resizer);
    let body = crate::pane_resize::main_areas(total)[0];
    let detail = crate::pane_resize::triptych_detail_area(body, false, false).unwrap();
    let cells = crate::pane_resize::detail_cell_areas(
        detail.inner(ratatui::layout::Margin::new(1, 1)),
        &app.detail_splits,
    );
    let inner = cells[1].inner(ratatui::layout::Margin::new(1, 1));
    assert_eq!(
        resizer.calls.lock().unwrap().as_slice(),
        [("t-hello-dev".into(), inner.width, inner.height)]
    );
    let rendered = render_to_buffer(&app, 120, 40);
    let text: String = rendered.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("developer output"));
}

#[test]
fn failed_resize_is_not_cached_and_retries_after_backoff() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FailsOnce(AtomicUsize);
    impl crate::pane_resize::PaneResizer for FailsOnce {
        fn resize(&self, _: &str, _: u16, _: u16) {}
        fn try_resize(&self, _: &str, _: u16, _: u16) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst) > 0
        }
    }
    let resizer = FailsOnce(AtomicUsize::new(0));
    let mut app = pane_sync_fixture();
    let area = Rect::new(0, 0, 120, 40);
    sync_focused_pane_size_to(&mut app, area, &resizer);
    assert!(app.last_synced_pane_sizes.is_empty());
    sync_focused_pane_size_to(&mut app, area, &resizer);
    assert_eq!(
        resizer.0.load(Ordering::SeqCst),
        1,
        "backoff avoids per-frame retries"
    );
    app.pane_resize_failures.get_mut("t-hello-mgr").unwrap().1 =
        Instant::now() - Duration::from_secs(2);
    sync_focused_pane_size_to(&mut app, area, &resizer);
    assert_eq!(resizer.0.load(Ordering::SeqCst), 2);
    assert!(app.last_synced_pane_sizes.contains_key("t-hello-mgr"));
    assert!(app.pane_resize_failures.is_empty());
}

#[test]
fn roster_and_compose_keep_selected_rows_visible() {
    let mut app = App::new();
    app.replace_team(fixture_team(
        (0..40)
            .map(|i| agent(&format!("p:agent-{i}"), AgentState::Running))
            .collect(),
    ));
    app.dismiss_splash();
    app.selected_agent = Some(39);
    let rendered = render_to_buffer(&app, 100, 24);
    let text: String = rendered.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("agent-39"));
    app.enter_compose_dm_for_focused();
    app.compose_editor.lines = (0..30).map(|i| format!("draft-{i}")).collect();
    app.compose_editor.cursor_row = 29;
    app.compose_editor.cursor_col = "draft-29".len();
    let rendered = render_to_buffer(&app, 100, 24);
    let text: String = rendered.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("draft-29▏"));
}

#[test]
fn configured_broker_path_drives_mailbox_and_approval_sources() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("team-compose.yaml"),
        "version: '2.0.0'\nbroker:\n  path: custom/mail.db\nprojects: []\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("custom")).unwrap();
    let path = dir.path().join("custom/mail.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE messages(id INTEGER PRIMARY KEY, sender TEXT, recipient TEXT, text TEXT, sent_at REAL);
        CREATE TABLE approvals(id INTEGER PRIMARY KEY, project_id TEXT, agent_id TEXT, action TEXT, summary TEXT, payload_json TEXT, status TEXT);
        CREATE TABLE channel_members(channel_id TEXT, agent_id TEXT);
        INSERT INTO messages VALUES(1, 'hello:dev', 'hello:mgr', 'custom database', 0);
        INSERT INTO approvals VALUES(2, 'hello', 'mgr', 'publish', 'custom approval', '', 'pending');").unwrap();
    let mut app = pane_sync_fixture();
    let mut team = fixture_team(vec![agent("hello:mgr", AgentState::Running)]);
    team.root = dir.path().to_path_buf();
    app.replace_team(team);
    assert_eq!(app.mailbox_db_path, path);
    assert_eq!(
        app.mailbox_db_path.parent().unwrap(),
        dir.path().join("custom")
    );
    let source = BrokerMailboxSource::new(app.mailbox_db_path.clone());
    refresh_mailbox(&mut app, &source);
    assert_eq!(app.mailbox.inbox[0].text, "custom database");
    refresh_approvals(&mut app, &BrokerApprovalSource::new(path));
    assert_eq!(app.pending_approvals[0].id, 2);
}

#[test]
fn terminal_and_snapshot_renderers_share_identical_output() {
    let mut app = pane_sync_fixture();
    app.pending_approvals = vec![ap(1)];
    for stage in [
        Stage::Triptych,
        Stage::StreamKeys,
        Stage::QuitConfirm,
        Stage::ApprovalsModal,
        Stage::HelpOverlay,
    ] {
        app.stage = stage;
        let expected = render_to_buffer(&app, 120, 40);
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert_eq!(terminal.backend().buffer(), &expected, "stage {stage:?}");
    }
}

#[test]
fn removing_a_preceding_split_preserves_focused_cell_and_removal_exits_streaming() {
    let mut app = pane_sync_fixture();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
        agent("p:c", AgentState::Running),
        agent("p:d", AgentState::Running),
    ]));
    app.detail_splits = vec![
        ("p:b".into(), SplitOrientation::Vertical),
        ("p:c".into(), SplitOrientation::Vertical),
        ("p:d".into(), SplitOrientation::Vertical),
        ("p:c".into(), SplitOrientation::Vertical),
    ];
    app.selected_split = 4;
    app.stage = Stage::StreamKeys;
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:c", AgentState::Running),
        agent("p:d", AgentState::Running),
    ]));
    assert_eq!(
        app.selected_split, 3,
        "duplicate C cell must retain its own position"
    );
    assert_eq!(app.stream_target_session().as_deref(), Some("t-p-c"));
    assert_eq!(app.stage, Stage::StreamKeys);
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:d", AgentState::Running),
    ]));
    assert_eq!(
        app.stage,
        Stage::Triptych,
        "never silently redirect live keystrokes"
    );
}

#[test]
fn wall_capture_and_input_target_match_the_labeled_roster_tile() {
    let mut app = pane_sync_fixture();
    app.replace_team(fixture_team(vec![
        agent("p:a", AgentState::Running),
        agent("p:b", AgentState::Running),
    ]));
    app.detail_splits = vec![("p:a".into(), SplitOrientation::Vertical)];
    app.selected_split = 1;
    app.selected_agent = Some(1);
    assert_eq!(app.stream_target_session().as_deref(), Some("t-p-a"));
    app.toggle_wall_layout();
    assert_eq!(app.stream_target_session().as_deref(), Some("t-p-b"));
    app.detail_buffer = vec!["visible runtime output".into()];
    app.team.channels = (0..30)
        .map(|i| channel(&format!("p:channel{i}"), "p"))
        .collect();
    app.selected_channel = Some(20);
    let text = |app: &App| {
        let buffer = render_to_buffer(app, 120, 24);
        buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
    };
    assert!(
        text(&app).contains("visible runtime output"),
        "channel cursor must not scroll Wall output"
    );
    app.toggle_mailbox_first_layout();
    assert!(
        text(&app).contains("#channel20"),
        "selected channel must remain visible"
    );
}

#[test]
fn explicit_or_environment_root_wins_over_a_different_team_in_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(a.join(".team")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    for root in [a.join(".team"), b.clone()] {
        std::fs::write(
            root.join("team-compose.yaml"),
            "version: 2.0.0\nprojects: []\n",
        )
        .unwrap();
    }
    let env = a.join(".team");
    let loaded = TeamSnapshot::load_selected(Some(&b), Some(env.as_os_str()), &a)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.root, b.canonicalize().unwrap());
    let loaded = TeamSnapshot::load_selected(None, Some(b.as_os_str()), &a)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.root, b.canonicalize().unwrap());
    assert!(TeamSnapshot::load_selected(Some(&tmp.path().join("missing")), None, &a).is_err());
}
