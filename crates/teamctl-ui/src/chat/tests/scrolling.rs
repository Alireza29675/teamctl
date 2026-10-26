use super::*;

fn app_with_evicted_tail(byte_cap: bool) -> App {
    let mut app = app();
    app.chat.focus = Focus::History;
    app.chat.thread_mut().unwrap().anchor = Some((1, 0));
    let count = if byte_cap { 65 } else { 1200 };
    let text = if byte_cap {
        "x".repeat(60_000)
    } else {
        "history".into()
    };
    for start in (1..=count).step_by(HISTORY_PAGE) {
        app.chat.apply(Response::History {
            conversation_id: "a".into(),
            before: None,
            result: Ok((start..=(start + HISTORY_PAGE as i64 - 1).min(count))
                .map(|id| message(id, "a", &text))
                .collect()),
        });
    }
    let thread = app.chat.thread_mut().unwrap();
    assert!(thread.tail_evicted());
    thread.unread = 7;
    app.chat.outbox.clear();
    app.chat_viewport = (80, 10);
    app
}

fn reach_cached_bottom(app: &mut App) {
    let (width, height) = app.chat_viewport;
    let anchor = {
        let rows = render::cached_rows(app.chat.thread().unwrap(), width);
        let row = &rows[rows.len().saturating_sub(height as usize)];
        (row.id, row.offset)
    };
    app.chat.thread_mut().unwrap().anchor = Some(anchor);
    key(app, KeyCode::Down);
}

fn assert_gap_free_tail_reload(byte_cap: bool) {
    let mut app = app_with_evicted_tail(byte_cap);
    let previous = app.chat.thread().unwrap().newest_seen.unwrap();
    let retained: Vec<_> = app
        .chat
        .thread()
        .unwrap()
        .messages
        .iter()
        .map(|message| message.id)
        .collect();
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: previous,
        result: Ok(vec![message(previous + 1, "a", "fresh reply")]),
    });
    // In the byte-capped case this short reply fits in the remaining budget.
    // It still cannot bridge the missing rows between history and the DB tail.
    assert_eq!(
        app.chat
            .thread()
            .unwrap()
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        retained
    );
    assert_eq!(app.chat.thread().unwrap().newest_seen, Some(previous + 1));
    assert_eq!(app.chat.thread().unwrap().unread, 8);
    reach_cached_bottom(&mut app);
    assert!(app.chat.thread().unwrap().reload_tail);
    assert!(app.chat.thread().unwrap().messages.is_empty());
    assert_eq!(app.chat.thread().unwrap().unread, 8);
    assert!(app.chat.outbox.iter().any(|request| matches!(request, Request::History { conversation_id, before: None } if conversation_id == "a")));

    // Work already queued against the historical window must not repopulate it.
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: Some(2),
        result: Ok(vec![message(1, "a", "old page")]),
    });
    app.chat.apply(Response::RefreshedHistory {
        conversation_id: "a".into(),
        result: Ok(vec![message(2, "a", "old status page")]),
    });
    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: previous + 1,
        result: Ok(vec![message(previous + 2, "a", "stale forward page")]),
    });
    assert!(app.chat.thread().unwrap().messages.is_empty());
    assert_eq!(app.chat.thread().unwrap().unread, 8);

    let start = (previous + 2 - HISTORY_PAGE as i64).max(1);
    let text = if byte_cap {
        "x".repeat(60_000)
    } else {
        "history".into()
    };
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok((start..=previous + 1)
            .map(|id| {
                message(
                    id,
                    "a",
                    if id == previous + 1 {
                        "fresh reply"
                    } else {
                        &text
                    },
                )
            })
            .collect()),
    });
    let thread = app.chat.thread().unwrap();
    assert!(!thread.reload_tail);
    assert_eq!(thread.unread, 0);
    assert_eq!(thread.anchor, None);
    assert_eq!(thread.messages.last().unwrap().id, previous + 1);
    assert!(thread
        .messages
        .windows(2)
        .all(|pair| pair[1].id == pair[0].id + 1));
    assert!(thread.messages.len() <= HISTORY_CAP);
    assert!(
        thread
            .messages
            .iter()
            .map(|message| message.text.len())
            .sum::<usize>()
            <= 2 * 1024 * 1024
    );

    app.chat.apply(Response::NewMessages {
        conversation_id: "a".into(),
        after: previous + 1,
        result: Ok(vec![message(previous + 2, "a", "reply after reload")]),
    });
    let thread = app.chat.thread().unwrap();
    assert_eq!(thread.messages.last().unwrap().text, "reply after reload");
    assert!(thread
        .messages
        .windows(2)
        .all(|pair| pair[1].id == pair[0].id + 1));
}

#[test]
fn scrolling_forward_after_message_cap_reloads_a_gap_free_tail() {
    assert_gap_free_tail_reload(false);
}

#[test]
fn scrolling_forward_after_byte_cap_reloads_a_gap_free_tail() {
    assert_gap_free_tail_reload(true);
}

#[test]
fn failed_tail_reload_keeps_unread_and_repeated_end_retries_once() {
    let mut app = app_with_evicted_tail(false);
    reach_cached_bottom(&mut app);
    app.chat.outbox.clear();
    key(&mut app, KeyCode::End);
    assert!(
        app.chat.outbox.is_empty(),
        "an in-flight reload must not be duplicated"
    );
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Err("database busy".into()),
    });
    assert_eq!(app.chat.thread().unwrap().unread, 7);
    assert!(app.chat.thread().unwrap().reload_tail);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::End);
    assert_eq!(app.chat.outbox.len(), 1);
    assert_eq!(app.chat.thread().unwrap().unread, 7);
    app.chat.apply(Response::History {
        conversation_id: "a".into(),
        before: None,
        result: Ok(vec![message(1200, "a", "database tail")]),
    });
    assert_eq!(app.chat.thread().unwrap().unread, 0);
    assert!(app.chat.thread().unwrap().error.is_none());
}

#[test]
fn own_send_during_evicted_history_keeps_the_cache_contiguous() {
    let mut app = app_with_evicted_tail(true);
    app.chat.focus = Focus::Composer;
    type_text(&mut app, "operator reply");
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
    let previous = app.chat.thread().unwrap().newest_seen;
    app.chat.apply(Response::Sent {
        conversation_id: "a".into(),
        request_id,
        result: Ok(message(66, "a", "operator reply")),
    });
    let thread = app.chat.thread().unwrap();
    assert_eq!(thread.newest_seen, previous);
    assert!(thread.tail_evicted());
    assert!(thread.editor.body().is_empty());
    assert!(thread.messages.iter().all(|message| message.id < 66));
    assert!(thread
        .messages
        .windows(2)
        .all(|pair| pair[1].id == pair[0].id + 1));
}
