use super::*;

#[test]
fn inbound_replay_survives_route_changes_and_mailbox_gc() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = inbound("peer", "source-1");
    let first = r.ingest(input.clone()).unwrap();
    let mut rerouted = input.clone();
    rerouted.agent_id = "p:b".into();
    rerouted.title = Some("new title".into());
    assert_eq!(r.ingest(rerouted).unwrap().id, first.id);
    conn.execute("DELETE FROM messages", []).unwrap();
    let retained = r.history(&first.conversation_id, None, None, 10).unwrap();
    assert_eq!(retained[0].mailbox_id, None);
    assert_eq!(retained[0].text, "hello");
    assert_eq!(r.ingest(input).unwrap().id, first.id);
    assert_eq!(count(&conn, "messages"), 0);
    assert_eq!(count(&conn, "chat_conversations"), 1);
}

#[test]
fn provider_source_scope_and_sender_conflicts_are_exact() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = inbound("peer", "1");
    r.ingest(input.clone()).unwrap();
    let mut different_sender = input.clone();
    different_sender.sender_id = "other".into();
    assert!(matches!(
        r.ingest(different_sender),
        Err(Error::IdempotencyConflict)
    ));
    let mut changed = input.clone();
    changed.text = "edited".into();
    assert!(matches!(r.ingest(changed), Err(Error::IdempotencyConflict)));
    let mut other_adapter = input;
    other_adapter.adapter_id = "whatsapp".into();
    r.ingest(other_adapter).unwrap();
    r.ingest(inbound("other-peer", "1")).unwrap();
    assert_eq!(count(&conn, "chat_conversations"), 3);
}

#[test]
fn inbound_never_originates_lifecycle_signals() {
    let conn = connection();
    let r = Repository::new(&conn);
    let mut input = inbound("peer", "1");
    input.kind = "system".into();
    assert!(matches!(r.ingest(input), Err(Error::InvalidInput(_))));
    let mut input = inbound("peer", "1");
    input.sender_id = "system:supervisor".into();
    let message = r.ingest(input).unwrap();
    let sender: String = conn
        .query_row(
            "SELECT sender FROM messages WHERE id = ?1",
            [message.mailbox_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(sender.starts_with("user:chat:"));
    assert_eq!(count(&conn, "chat_messages"), 1);
}

#[test]
fn failed_ingress_leaves_no_orphan_route_or_mailbox() {
    let conn = connection();
    let r = Repository::new(&conn);
    conn.execute_batch("CREATE TRIGGER reject_chat BEFORE INSERT ON chat_messages BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
    assert!(r.ingest(inbound("peer", "1")).is_err());
    for table in [
        "messages",
        "chat_messages",
        "chat_conversations",
        "chat_sources",
    ] {
        assert_eq!(count(&conn, table), 0);
    }
    assert!(conn.is_autocommit());
}

#[test]
fn native_media_publication_keeps_callers_transaction_and_legacy_id() {
    let conn = connection();
    let r = Repository::new(&conn);
    let mut input = inbound("peer", "media-42");
    input.kind = "image".into();
    input.payload = Some(serde_json::json!({"path":"/private/photo.png"}));
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let id = existing(&conn, &input);
    let message = r.record_existing_inbound(id, input).unwrap();
    assert!(!conn.is_autocommit());
    let (external, telegram, chat): (String, i64, i64) = conn
        .query_row(
            "SELECT external_id, telegram_msg_id, chat_message_id FROM messages WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (external.as_str(), telegram, chat),
        ("media-42", 42, message.id)
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(count(&conn, "messages"), 0);
    assert_eq!(count(&conn, "chat_messages"), 0);
}

#[test]
fn hidden_pending_media_cannot_gain_a_visible_route() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = inbound("peer", "42");
    let id = existing(&conn, &input);
    conn.execute(
        "UPDATE messages SET kind = 'media_pending' WHERE id = ?1",
        [id],
    )
    .unwrap();
    assert!(matches!(
        r.record_existing_inbound(id, input),
        Err(Error::Forbidden)
    ));
    assert_eq!(count(&conn, "chat_messages"), 0);
}

#[test]
fn native_duplicate_cleanup_only_deletes_matching_untouched_rows() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = inbound("peer", "42");
    let original_id = existing(&conn, &input);
    let original = r
        .record_existing_inbound(original_id, input.clone())
        .unwrap();
    let duplicate = existing(&conn, &input);
    assert_eq!(
        r.record_existing_inbound(duplicate, input.clone())
            .unwrap()
            .id,
        original.id
    );
    assert_eq!(count(&conn, "messages"), 1);
    for attribute in [
        "acked_at = 2",
        "delivered_at = 2",
        "chat_message_id = 999",
        "conversation_id = 'other'",
    ] {
        let id = existing(&conn, &input);
        conn.execute(
            &format!("UPDATE messages SET {attribute} WHERE id = ?1"),
            [id],
        )
        .unwrap();
        assert!(matches!(
            r.record_existing_inbound(id, input.clone()),
            Err(Error::IdempotencyConflict)
        ));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages WHERE id = ?1", [id], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }
    let id = existing(&conn, &input);
    conn.execute("UPDATE messages SET text = 'other' WHERE id = ?1", [id])
        .unwrap();
    assert!(matches!(
        r.record_existing_inbound(id, input),
        Err(Error::IdempotencyConflict)
    ));
}

#[test]
fn nested_duplicate_cleanup_can_be_rolled_back_by_native_adapter() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = inbound("peer", "42");
    r.ingest(input.clone()).unwrap();
    let id = existing(&conn, &input);
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    r.record_existing_inbound(id, input).unwrap();
    assert_eq!(count(&conn, "messages"), 1);
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(count(&conn, "messages"), 2);
}
