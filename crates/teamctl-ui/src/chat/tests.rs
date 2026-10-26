use super::*;
use crate::app::{handle_event, render_to_buffer, App, Stage};
use crate::approvals::test_support::MockApprovalDecider;
use crate::compose::test_support::MockMessageSender;
use crate::data::{AgentInfo, TeamSnapshot};
use crate::keysender::test_support::MockKeySender;
use crate::mailbox::test_support::MockMailboxSource;
use crate::theme::{Capabilities, ColorMode};
use crate::triptych::{MainLayout, Pane};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use team_core::chat::{ChatMessage, Conversation, DeliveryStatus, Direction, Repository};

mod scrolling;

fn conversation(id: &str, agent: &str) -> Conversation {
    Conversation {
        id: id.into(),
        adapter_id: "local".into(),
        peer_id: "operator".into(),
        agent_id: agent.into(),
        title: agent.into(),
        created_at: 0.0,
        updated_at: 0.0,
        revision: 1,
    }
}

fn message(id: i64, conversation: &str, text: &str) -> ChatMessage {
    ChatMessage {
        id,
        conversation_id: conversation.into(),
        direction: Direction::Human,
        text: text.into(),
        kind: "text".into(),
        payload: None,
        mailbox_id: None,
        external_id: None,
        status: DeliveryStatus::Received,
        error: None,
        created_at: 0.0,
        updated_at: 0.0,
        revision: 1,
    }
}

fn agent(id: &str) -> AgentInfo {
    let (project, name) = id.split_once(':').unwrap();
    AgentInfo {
        id: id.into(),
        agent: name.into(),
        project: project.into(),
        tmux_session: format!("t-{project}-{name}"),
        state: team_core::supervisor::AgentState::Stopped,
        unread_mail: 0,
        pending_approvals: 0,
        is_manager: false,
        display_name: None,
        rate_limit_resets_at: None,
        last_activity_at: None,
        reports_to: None,
    }
}

fn app() -> App {
    let mut app = App::new();
    let mut team = TeamSnapshot::empty(std::path::PathBuf::from("/tmp/chat-fixture/.team"));
    team.agents = vec![agent("p:a"), agent("p:b")];
    app.replace_team(team);
    app.stage = Stage::Triptych;
    app.layout = MainLayout::Chat;
    app.capabilities = Capabilities {
        color: ColorMode::Monochrome,
    };
    app.chat.upsert_conversation(conversation("a", "p:a"));
    app.chat.upsert_conversation(conversation("b", "p:b"));
    app.chat.select(Selection::Conversation("a".into()));
    app.chat.outbox.clear();
    app
}

fn event(app: &mut App, event: Event) {
    handle_event(
        app,
        event,
        &MockApprovalDecider::default(),
        &MockMessageSender::default(),
        &MockMailboxSource::default(),
        &MockKeySender::default(),
    );
}

fn key(app: &mut App, code: KeyCode) {
    event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
fn modified(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    event(app, Event::Key(KeyEvent::new(code, modifiers)));
}
fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        key(app, KeyCode::Char(character));
    }
}

#[test]
fn composer_keeps_shortcuts_literal_including_after_escape() {
    let mut app = app();
    type_text(&mut app, "q c @ ! سلام é 📎");
    key(&mut app, KeyCode::Esc);
    type_text(&mut app, "q");
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.chat.thread().unwrap().editor.lines,
        ["q c @ ! سلام é 📎q", ""]
    );
    assert_eq!(app.stage, Stage::Triptych);
    assert_eq!(app.layout, MainLayout::Chat);
}

#[test]
fn drafts_follow_identity_across_two_threads_and_refreshes() {
    let mut app = app();
    type_text(&mut app, "first");
    app.chat.select(Selection::Conversation("b".into()));
    type_text(&mut app, "second");
    app.chat.apply(Response::Listed(Ok(vec![
        conversation("b", "p:b"),
        conversation("a", "p:a"),
    ])));
    app.chat.select(Selection::Conversation("a".into()));
    assert_eq!(app.chat.thread().unwrap().editor.body(), "first");
    app.chat.select(Selection::Conversation("b".into()));
    assert_eq!(app.chat.thread().unwrap().editor.body(), "second");
}

#[test]
fn all_three_send_chords_preserve_draft_until_success() {
    for chord in [KeyModifiers::ALT, KeyModifiers::CONTROL, KeyModifiers::NONE] {
        let mut app = app();
        type_text(&mut app, "hello");
        if chord.is_empty() {
            key(&mut app, KeyCode::Esc);
        }
        modified(&mut app, KeyCode::Enter, chord);
        let submission = app.chat.thread().unwrap().submission.as_ref().unwrap();
        assert!(submission.pending);
        assert_eq!(submission.text, "hello");
        assert_eq!(app.chat.thread().unwrap().editor.body(), "hello");
        assert!(
            matches!(app.chat.outbox.back(), Some(Request::Send { conversation_id, text, .. }) if conversation_id == "a" && text == "hello")
        );
    }
}

#[test]
fn delayed_failure_is_scoped_and_retry_reuses_request_identity() {
    let mut app = app();
    type_text(&mut app, "retry me");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let request_id = app
        .chat
        .thread()
        .unwrap()
        .submission
        .as_ref()
        .unwrap()
        .request_id
        .clone();
    app.chat.select(Selection::Conversation("b".into()));
    type_text(&mut app, "different draft");
    app.chat.apply(Response::Sent {
        conversation_id: "a".into(),
        request_id: request_id.clone(),
        result: Err("database busy".into()),
    });
    assert_eq!(app.chat.selected_id(), Some("b"));
    assert_eq!(app.chat.thread().unwrap().editor.body(), "different draft");
    app.chat.select(Selection::Conversation("a".into()));
    assert_eq!(app.chat.thread().unwrap().editor.body(), "retry me");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(
        app.chat
            .thread()
            .unwrap()
            .submission
            .as_ref()
            .unwrap()
            .request_id,
        request_id
    );
}

#[test]
fn success_does_not_erase_a_draft_edited_while_pending() {
    let mut app = app();
    type_text(&mut app, "hello");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let request_id = app
        .chat
        .thread()
        .unwrap()
        .submission
        .as_ref()
        .unwrap()
        .request_id
        .clone();
    type_text(&mut app, " new draft");
    app.chat.apply(Response::Sent {
        conversation_id: "a".into(),
        request_id,
        result: Ok(message(1, "a", "hello")),
    });
    assert_eq!(app.chat.thread().unwrap().editor.body(), "hello new draft");
    assert!(app.chat.thread().unwrap().submission.is_none());
}

#[test]
fn stale_submission_result_cannot_clear_a_new_retry() {
    let mut app = app();
    type_text(&mut app, "hello");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let current = app
        .chat
        .thread()
        .unwrap()
        .submission
        .as_ref()
        .unwrap()
        .request_id
        .clone();
    app.chat.apply(Response::Sent {
        conversation_id: "a".into(),
        request_id: "old-request".into(),
        result: Ok(message(1, "a", "old")),
    });
    assert!(
        app.chat
            .thread()
            .unwrap()
            .submission
            .as_ref()
            .unwrap()
            .pending
    );
    assert_eq!(
        app.chat
            .thread()
            .unwrap()
            .submission
            .as_ref()
            .unwrap()
            .request_id,
        current
    );
}

#[test]
fn delayed_open_never_retargets_a_selected_conversation() {
    let mut app = app();
    let epoch = app.chat.selection_epoch;
    app.chat.open_agent("p:a".into());
    app.chat.select(Selection::Conversation("b".into()));
    app.chat.apply(Response::Opened {
        selection_epoch: epoch,
        result: Ok(conversation("new-a", "p:a")),
    });
    assert_eq!(app.chat.selected_id(), Some("b"));
    assert!(app
        .chat
        .conversations
        .iter()
        .any(|conversation| conversation.id == "new-a"));
    app.chat.error = Some("current thread error".into());
    app.chat.apply(Response::Opened {
        selection_epoch: epoch,
        result: Err("old open failed".into()),
    });
    assert_eq!(app.chat.error.as_deref(), Some("current thread error"));
}

#[test]
fn switching_to_another_agent_can_open_it_while_the_first_open_is_pending() {
    let mut app = app();
    let first_epoch = app.chat.selection_epoch;
    app.chat.open_agent("p:a".into());
    app.chat.select(Selection::NewAgent("p:b".into()));
    let second_epoch = app.chat.selection_epoch;
    app.chat.open_agent("p:b".into());
    assert_eq!(
        app.chat
            .outbox
            .iter()
            .filter(|request| matches!(request, Request::Open { .. }))
            .count(),
        2
    );
    app.chat.apply(Response::Opened {
        selection_epoch: first_epoch,
        result: Ok(conversation("new-a", "p:a")),
    });
    assert!(app.chat.open_pending);
    assert_eq!(app.chat.selection, Some(Selection::NewAgent("p:b".into())));
    app.chat.apply(Response::Opened {
        selection_epoch: second_epoch,
        result: Ok(conversation("new-b", "p:b")),
    });
    assert!(!app.chat.open_pending);
    assert_eq!(app.chat.selected_id(), Some("new-b"));
}

#[test]
fn history_status_refreshes_without_new_ids_and_ignores_old_revisions() {
    let mut app = app();
    let mut row = message(1, "a", "queued");
    row.status = DeliveryStatus::Queued;
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![row.clone()]),
    });
    row.status = DeliveryStatus::Failed;
    row.error = Some("disconnected".into());
    row.revision = 2;
    app.chat.apply(Response::RefreshedHistory {
        conversation_id: "a".into(),
        result: Ok(vec![row.clone()]),
    });
    row.status = DeliveryStatus::Queued;
    row.revision = 1;
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![row]),
    });
    assert_eq!(
        app.chat.thread().unwrap().messages[0].status,
        DeliveryStatus::Failed
    );
    assert_eq!(
        app.chat.thread().unwrap().messages[0].error.as_deref(),
        Some("disconnected")
    );
}

#[test]
fn older_results_and_new_arrivals_preserve_historical_anchor() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(2, "a", "two"), message(3, "a", "three")]),
    });
    app.chat.thread_mut().unwrap().anchor = Some((2, 1));
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: Some(2),
        result: Ok(vec![message(1, "a", "one")]),
    });
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(4, "a", "new")]),
    });
    assert_eq!(app.chat.thread().unwrap().anchor, Some((2, 1)));
    assert_eq!(app.chat.thread().unwrap().unread, 1);
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(4, "a", "new")]),
    });
    assert_eq!(app.chat.thread().unwrap().unread, 1);
    key(&mut app, KeyCode::End);
    assert_eq!(app.chat.thread().unwrap().anchor, None);
    assert_eq!(app.chat.thread().unwrap().unread, 1);
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(4, "a", "new")]),
    });
    assert_eq!(app.chat.thread().unwrap().unread, 0);
}

#[test]
fn page_up_requests_older_sql_page_and_refreshes_visible_statuses() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok((100..200).map(|id| message(id, "a", "line")).collect()),
    });
    app.chat_viewport = (80, 1000);
    key(&mut app, KeyCode::PageUp);
    assert!(app.chat.outbox.iter().any(|request| matches!(request, Request::History { conversation_id, before: Some(100) } if conversation_id == "a")));
    app.chat.thread_mut().unwrap().refreshing = false;
    app.chat.refresh_selected();
    assert!(app.chat.outbox.iter().any(|request| matches!(request, Request::RefreshHistory { conversation_id, after: 99 } if conversation_id == "a")));
}

#[test]
fn removed_agents_and_remote_threads_keep_history_but_disable_send() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1, "a", "retained")]),
    });
    app.team.agents.clear();
    type_text(&mut app, "blocked");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert!(app.chat.thread().unwrap().editor.is_empty());
    assert_eq!(app.chat.thread().unwrap().messages[0].text, "retained");
    let text = rendered(&app, 120, 30);
    assert!(text.contains("Agent removed"));
    let mut remote = conversation("remote", "p:a");
    remote.adapter_id = "telegram".into();
    remote.peer_id = "peer-7".into();
    app.chat.upsert_conversation(remote);
    app.chat.select(Selection::Conversation("remote".into()));
    type_text(&mut app, "cannot impersonate");
    assert!(app.chat.thread().unwrap().editor.is_empty());
    assert!(rendered(&app, 120, 30).contains("Remote history"));
}

#[test]
fn chat_exit_restores_dashboard_layout_and_selection() {
    let mut app = app();
    app.layout = MainLayout::Wall;
    app.selected_agent = Some(1);
    app.focused_pane = Pane::Mailbox;
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.layout, MainLayout::Chat);
    modified(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(app.layout, MainLayout::Wall);
    assert_eq!(app.selected_agent, Some(1));
    assert_eq!(app.focused_pane, Pane::Mailbox);
}

#[test]
fn bounded_queue_rejects_overflow_without_marking_an_unsent_draft_pending() {
    let mut app = app();
    for _ in 0..QUEUE_CAP {
        app.chat.enqueue(Request::List);
    }
    type_text(&mut app, "held draft");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(app.chat.outbox.len(), QUEUE_CAP);
    assert!(app.chat.thread().unwrap().submission.is_none());
    assert_eq!(app.chat.thread().unwrap().editor.body(), "held draft");
    assert!(app.chat.error.as_ref().unwrap().contains("busy"));
}

#[test]
fn loaded_history_is_bounded_and_status_does_not_depend_on_mailbox_rows() {
    let mut app = app();
    for page in 0..12 {
        app.chat.apply(Response::History {
            conversation_id: "a".into(),
            before: None,
            result: Ok((page * 100..(page + 1) * 100)
                .map(|id| message(id, "a", "history"))
                .collect()),
        });
    }
    assert_eq!(app.chat.thread().unwrap().messages.len(), HISTORY_CAP);
    assert!(app
        .chat
        .thread()
        .unwrap()
        .messages
        .iter()
        .all(|message| message.mailbox_id.is_none()));
    assert!(app.chat.thread().unwrap().has_older);
}

#[test]
fn forward_pages_drain_backlog_without_skipping_or_accepting_stale_cursors() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1, "a", "start")]),
    });
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: 1,
        result: Ok((2..102).map(|id| message(id, "a", "backlog")).collect()),
    });
    assert_eq!(app.chat.thread().unwrap().messages.len(), 101);
    assert!(app.chat.outbox.iter().any(|request| matches!(request, Request::NewMessages { conversation_id, after: 101 } if conversation_id == "a")));
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: 1,
        result: Ok(vec![message(9999, "a", "stale")]),
    });
    assert_eq!(app.chat.thread().unwrap().newest_seen, Some(101));
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: 101,
        result: Ok(vec![message(102, "a", "last")]),
    });
    assert_eq!(app.chat.thread().unwrap().messages.len(), 102);
    assert!(!app.chat.thread().unwrap().new_pending);
}

#[test]
fn accepted_own_send_does_not_advance_the_unread_database_cursor() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1, "a", "start")]),
    });
    type_text(&mut app, "mine");
    modified(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let request_id = app
        .chat
        .thread()
        .unwrap()
        .submission
        .as_ref()
        .unwrap()
        .request_id
        .clone();
    app.chat.apply(Response::Sent {
        conversation_id: "a".into(),
        request_id,
        result: Ok(message(200, "a", "mine")),
    });
    assert_eq!(app.chat.thread().unwrap().newest_seen, Some(1));
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: 1,
        result: Ok(vec![message(2, "a", "unseen reply")]),
    });
    assert!(app
        .chat
        .thread()
        .unwrap()
        .messages
        .iter()
        .any(|message| message.id == 2));
}

#[test]
fn text_byte_cap_preserves_the_viewed_message_and_older_paging() {
    let mut app = app();
    app.chat.thread_mut().unwrap().anchor = Some((100, 1));
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok((1..102)
            .map(|id| message(id, "a", &"x".repeat(65536)))
            .collect()),
    });
    let thread = app.chat.thread().unwrap();
    assert!(thread.messages.iter().any(|message| message.id == 100));
    assert!(
        thread
            .messages
            .iter()
            .map(|message| message.text.len())
            .sum::<usize>()
            <= 2 * 1024 * 1024
    );
    assert!(thread.has_older);
    assert_eq!(thread.anchor, Some((100, 1)));
}

#[test]
fn remote_peers_are_distinct_in_sidebar_and_full_thread_heading() {
    let mut app = app();
    for peer in ["alice-opaque-7", "bob-opaque-9"] {
        let mut remote = conversation(peer, "p:a");
        remote.adapter_id = "telegram".into();
        remote.peer_id = peer.into();
        app.chat.upsert_conversation(remote);
    }
    app.chat
        .select(Selection::Conversation("alice-opaque-7".into()));
    let view = rendered(&app, 120, 30);
    assert!(view.contains("alice-opaque-7"));
    assert!(view.contains("bob-opaque-9"));
    assert!(view.contains("p:a · telegram · alice-opaque-7"));
}

#[test]
fn cached_transcript_updates_status_without_rebuilding_unchanged_rows() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1, "a", "hello")]),
    });
    assert!(rendered(&app, 120, 30).contains("received"));
    let mut changed = message(1, "a", "hello");
    changed.status = DeliveryStatus::Failed;
    changed.error = Some("transport unavailable".into());
    changed.revision = 2;
    app.chat.apply(Response::RefreshedHistory {
        conversation_id: "a".into(),
        result: Ok(vec![changed]),
    });
    let view = rendered(&app, 120, 30);
    assert!(view.contains("failed"));
    assert!(view.contains("transport unavailable"));
}

#[test]
fn recovered_list_failure_clears_only_its_error() {
    let mut app = app();
    app.chat.error = Some("An open request failed".into());
    app.chat
        .apply(Response::Listed(Err("database busy".into())));
    assert_eq!(app.chat.list_error.as_deref(), Some("database busy"));
    app.chat
        .apply(Response::Listed(Ok(vec![conversation("a", "p:a")])));
    assert!(app.chat.list_error.is_none());
    assert_eq!(app.chat.error.as_deref(), Some("An open request failed"));
}

#[test]
fn unicode_paste_is_bounded_without_splitting_utf8() {
    let mut app = app();
    event(&mut app, Event::Paste("é".repeat(40000)));
    assert_eq!(
        app.chat.thread().unwrap().editor.body().len(),
        team_core::chat::MAX_TEXT_BYTES
    );
    key(&mut app, KeyCode::Char('📎'));
    assert_eq!(
        app.chat.thread().unwrap().editor.body().len(),
        team_core::chat::MAX_TEXT_BYTES
    );
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        app.chat.thread().unwrap().editor.body().len(),
        team_core::chat::MAX_TEXT_BYTES - 2
    );
}

fn rendered(app: &App, width: u16, height: u16) -> String {
    let buffer = render_to_buffer(app, width, height);
    let mut output = String::new();
    for y in 0..height {
        for x in 0..width {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

#[test]
fn chat_snapshot_wide_and_narrow() {
    let mut app = app();
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1, "a", "Hello from the operator."), {
            let mut reply = message(
                2,
                "a",
                "سلام é 📎 — replies remain in this conversation after mailbox cleanup.",
            );
            reply.direction = Direction::Agent;
            reply.status = DeliveryStatus::Sent;
            reply
        }]),
    });
    type_text(&mut app, "Draft q c @ !");
    insta::assert_snapshot!("chat_120x30", rendered(&app, 120, 30));
    insta::assert_snapshot!("chat_50x18", rendered(&app, 50, 18));
    for (width, height) in [(1, 1), (0, 0), (8, 4), (40, 10)] {
        rendered(&app, width, height);
    }
}

fn receive(worker: &ChatWorker) -> Response {
    let start = std::time::Instant::now();
    loop {
        if let Some(response) = worker.try_receive() {
            return response;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "worker response timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn worker_round_trip_reopens_cli_identity_and_retains_history_after_gc() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mailbox.db");
    let connection = team_core::chat::open(&path).unwrap();
    connection.execute_batch("INSERT INTO projects(id,name) VALUES ('p','P'); INSERT INTO agents(id,project_id,role,runtime) VALUES ('p:a','p','worker','fake');").unwrap();
    let repository = Repository::new(&connection);
    let conversation = repository.open_local("p:a").unwrap();
    let human = repository
        .send_local(&conversation.id, "CLI hello", "cli-request")
        .unwrap();
    repository
        .reply(
            "p:a",
            Some(&conversation.id),
            human.mailbox_id,
            "Agent reply",
            "text",
            None,
            Some("reply-request"),
        )
        .unwrap();
    connection.execute("DELETE FROM messages", []).unwrap();
    let worker = ChatWorker::new(path.clone());
    worker
        .try_submit(Request::Open {
            agent_id: "p:a".into(),
            selection_epoch: 0,
        })
        .unwrap();
    let Response::Opened {
        result: Ok(opened), ..
    } = receive(&worker)
    else {
        panic!("expected opened conversation")
    };
    assert_eq!(opened.id, conversation.id);
    worker
        .try_submit(Request::History {
            conversation_id: opened.id.clone(),
            before: None,
        })
        .unwrap();
    let Response::History {
        result: Ok(rows), ..
    } = receive(&worker)
    else {
        panic!("expected history")
    };
    assert_eq!(
        rows.iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["CLI hello", "Agent reply"]
    );
    assert!(rows.iter().all(|message| message.mailbox_id.is_none()));
    drop(worker);
    let reopened = ChatWorker::new(path);
    reopened
        .try_submit(Request::History {
            conversation_id: opened.id,
            before: None,
        })
        .unwrap();
    let Response::History {
        result: Ok(rows), ..
    } = receive(&reopened)
    else {
        panic!("expected reopened history")
    };
    assert_eq!(rows.len(), 2);
}
