use super::chat::ReplyPart;
use super::*;
use serde_json::json;
use team_core::chat::{ChatMessage, InboundMessage, Repository};
use tempfile::NamedTempFile;

fn chat_store() -> (Store, NamedTempFile) {
    let file = NamedTempFile::new().unwrap();
    let store = Store::open(file.path()).unwrap();
    for agent in ["p:dev", "p:other"] {
        store
            .upsert_agent(agent, "p", "P", "developer", "codex", false)
            .unwrap();
    }
    (store, file)
}

fn inbound(store: &Store, adapter: &str, peer: &str, agent: &str, external: &str) -> ChatMessage {
    let conn = store.conn.lock().unwrap();
    Repository::new(&conn)
        .ingest(InboundMessage {
            adapter_id: adapter.into(),
            peer_id: peer.into(),
            sender_id: peer.into(),
            agent_id: agent.into(),
            external_id: external.into(),
            text: "hello".into(),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap()
}

fn text(value: &str) -> ReplyPart {
    ReplyPart {
        text: value.into(),
        kind: "text",
        payload: None,
    }
}

fn image(caption: &str) -> ReplyPart {
    ReplyPart {
        text: caption.into(),
        kind: "image",
        payload: Some(json!({"source": "url", "value": "https://example.com/image.png"})),
    }
}

fn register(store: &Store, adapter: &str, kinds: &[&str]) {
    let conn = store.conn.lock().unwrap();
    Repository::new(&conn)
        .register_capabilities(
            adapter,
            &kinds.iter().map(|kind| (*kind).into()).collect::<Vec<_>>(),
        )
        .unwrap();
}

fn counts(store: &Store) -> (i64, i64, i64, i64, i64) {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM messages), (SELECT COUNT(*) FROM chat_messages),
        (SELECT COUNT(*) FROM chat_deliveries), (SELECT COUNT(*) FROM chat_requests),
        (SELECT COALESCE(SUM(revision),0) FROM chat_conversations)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

#[test]
fn invalid_later_part_rolls_back_the_entire_reply_and_its_idempotency_receipts() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    let before = counts(&store);
    let error = store
        .reply_chat(
            "p:dev",
            Some(&source.conversation_id),
            source.mailbox_id,
            Some("batch"),
            &[text("first must roll back"), text("bad\0text")],
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("invalid chat input"), "{error}");
    assert_eq!(counts(&store), before);
    let retry = store
        .reply_chat(
            "p:dev",
            Some(&source.conversation_id),
            source.mailbox_id,
            Some("batch"),
            &[text("corrected first"), text("corrected second")],
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        retry.len(),
        2,
        "a rolled-back attempt must not reserve its request ID"
    );
}

#[test]
fn database_failure_midway_rolls_back_delivery_rows_receipts_and_conversation_revision() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    register(&store, "fixture", &["text", "image"]);
    store.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_chat_image BEFORE INSERT ON chat_messages
        WHEN NEW.direction='agent' AND NEW.kind='image' BEGIN SELECT RAISE(ABORT, 'fixture disk failure'); END;").unwrap();
    let before = counts(&store);
    let parts = [text("first"), image("second")];
    let error = store
        .reply_chat(
            "p:dev",
            Some(&source.conversation_id),
            source.mailbox_id,
            Some("batch"),
            &parts,
        )
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("fixture disk failure"),
        "{error}"
    );
    assert_eq!(counts(&store), before);
    store
        .conn
        .lock()
        .unwrap()
        .execute("DROP TRIGGER fail_chat_image", [])
        .unwrap();
    assert_eq!(
        store
            .reply_chat(
                "p:dev",
                Some(&source.conversation_id),
                source.mailbox_id,
                Some("batch"),
                &parts
            )
            .unwrap()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn multipart_retry_after_gc_returns_the_original_ids_even_when_a_second_route_exists() {
    let (store, file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    register(&store, "fixture", &["text", "image"]);
    let parts = [text("first"), image("second")];
    let first = store
        .reply_chat("p:dev", None, source.mailbox_id, Some("batch"), &parts)
        .unwrap()
        .unwrap();
    store
        .inbox_ack("p:dev", &[source.mailbox_id.unwrap()])
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM messages WHERE acked_at IS NOT NULL", [])
        .unwrap();
    inbound(&store, "fixture", "bob", "p:dev", "b");
    let before = counts(&store);
    drop(store);
    let reopened = Store::open(file.path()).unwrap();
    let retried = reopened
        .reply_chat("p:dev", None, source.mailbox_id, Some("batch"), &parts)
        .unwrap()
        .unwrap();
    assert_eq!(retried, first);
    assert_eq!(counts(&reopened), before);
    let conn = reopened.conn.lock().unwrap();
    let history = Repository::new(&conn)
        .history(&source.conversation_id, None, None, 20)
        .unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].mailbox_id, None);
}

#[test]
fn whole_request_receipt_rejects_removing_reordering_or_changing_a_part() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    register(&store, "fixture", &["text", "image"]);
    store
        .reply_chat(
            "p:dev",
            Some(&source.conversation_id),
            None,
            Some("batch"),
            &[text("first"), image("second")],
        )
        .unwrap();
    let before = counts(&store);
    for parts in [
        vec![text("first")],
        vec![image("second"), text("first")],
        vec![text("first"), image("changed")],
    ] {
        let error = store
            .reply_chat(
                "p:dev",
                Some(&source.conversation_id),
                None,
                Some("batch"),
                &parts,
            )
            .err()
            .unwrap();
        assert!(matches!(
            error.downcast_ref::<team_core::chat::Error>(),
            Some(team_core::chat::Error::IdempotencyConflict)
        ));
        assert_eq!(counts(&store), before);
    }
}

#[test]
fn multipart_receipt_cannot_be_redirected_to_another_owned_conversation() {
    let (store, _file) = chat_store();
    let alice = inbound(&store, "fixture", "alice", "p:dev", "a");
    let bob = inbound(&store, "fixture", "bob", "p:dev", "b");
    store
        .reply_chat(
            "p:dev",
            Some(&alice.conversation_id),
            None,
            Some("same-request"),
            &[text("hello")],
        )
        .unwrap();
    let before = counts(&store);
    assert!(store
        .reply_chat(
            "p:dev",
            Some(&bob.conversation_id),
            None,
            Some("same-request"),
            &[text("hello")]
        )
        .is_err());
    assert_eq!(counts(&store), before);
}

#[test]
fn missing_or_text_only_capabilities_reject_media_without_a_partial_text_reply() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    for declared in [false, true] {
        if declared {
            register(&store, "fixture", &["text"]);
        }
        let before = counts(&store);
        assert!(store
            .reply_chat(
                "p:dev",
                Some(&source.conversation_id),
                None,
                Some("batch"),
                &[text("first"), image("second")]
            )
            .is_err());
        assert_eq!(counts(&store), before);
    }
}

#[test]
fn generic_text_uses_the_shared_limit_while_telegram_caps_use_utf16_units() {
    let (store, _file) = chat_store();
    let generic = inbound(&store, "fixture", "alice", "p:dev", "a");
    assert_eq!(
        store
            .reply_chat(
                "p:dev",
                Some(&generic.conversation_id),
                None,
                None,
                &[text(&"x".repeat(4097))]
            )
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    let telegram = inbound(&store, "telegram:fixture", "alice", "p:dev", "1");
    register(&store, "telegram:fixture", &["text", "image"]);
    assert_eq!(
        store
            .reply_chat(
                "p:dev",
                Some(&telegram.conversation_id),
                None,
                None,
                &[text(&"📎".repeat(2048)), image(&"📎".repeat(512))]
            )
            .unwrap()
            .unwrap()
            .len(),
        2
    );
    for parts in [
        vec![text(&"📎".repeat(2049))],
        vec![text("valid first"), image(&"📎".repeat(513))],
    ] {
        let before = counts(&store);
        let error = store
            .reply_chat("p:dev", Some(&telegram.conversation_id), None, None, &parts)
            .err()
            .unwrap();
        assert!(error.to_string().contains("Telegram"), "{error}");
        assert_eq!(counts(&store), before);
    }
}

#[test]
fn foreign_agent_or_nonhuman_mailbox_ids_cannot_use_legacy_fallback() {
    let (store, _file) = chat_store();
    let foreign = inbound(&store, "fixture", "alice", "p:other", "a");
    let internal = store
        .send_dm("p", "p:other", "p:dev", "internal", None, None)
        .unwrap();
    let before = counts(&store);
    for mailbox in [foreign.mailbox_id.unwrap(), internal] {
        assert!(store
            .reply_chat("p:dev", None, Some(mailbox), None, &[text("must not send")])
            .is_err());
        assert_eq!(counts(&store), before);
    }
}

#[test]
fn explicit_legacy_reference_is_not_captured_by_an_unrelated_local_route() {
    let (store, _file) = chat_store();
    let legacy = store
        .send_dm("p", "user:telegram", "p:dev", "old", None, None)
        .unwrap();
    {
        let conn = store.conn.lock().unwrap();
        Repository::new(&conn).open_local("p:dev").unwrap();
    }
    let before = counts(&store);
    assert!(store
        .reply_chat("p:dev", None, Some(legacy), None, &[text("legacy reply")])
        .unwrap()
        .is_none());
    assert_eq!(counts(&store), before);
}

#[test]
fn invalid_request_ids_leave_no_partial_reply_or_receipt() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    let before = counts(&store);
    for request in [String::new(), "x".repeat(1001), "bad\nrequest".into()] {
        assert!(store
            .reply_chat(
                "p:dev",
                Some(&source.conversation_id),
                None,
                Some(&request),
                &[text("hello")]
            )
            .is_err());
        assert_eq!(counts(&store), before);
    }
    assert_eq!(
        store
            .reply_chat(
                "p:dev",
                Some(&source.conversation_id),
                None,
                Some(&"x".repeat(1000)),
                &[text("hello")]
            )
            .unwrap()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn hidden_or_half_linked_inbound_rows_cannot_fall_back_to_the_legacy_queue() {
    let (store, _file) = chat_store();
    let source = inbound(&store, "fixture", "alice", "p:dev", "a");
    for mutation in [
        "UPDATE messages SET kind='media_pending' WHERE id=?1",
        "UPDATE messages SET kind='text', chat_message_id=NULL WHERE id=?1",
        "UPDATE messages SET conversation_id=NULL, chat_message_id=42 WHERE id=?1",
    ] {
        store
            .conn
            .lock()
            .unwrap()
            .execute(mutation, [source.mailbox_id.unwrap()])
            .unwrap();
        let before = counts(&store);
        assert!(store
            .reply_chat(
                "p:dev",
                None,
                source.mailbox_id,
                None,
                &[text("must not escape")]
            )
            .is_err());
        assert_eq!(counts(&store), before);
    }
}
