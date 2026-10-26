use super::*;

#[test]
fn local_identity_is_stable_and_submission_ids_detect_conflicts() {
    let conn = connection();
    let r = Repository::new(&conn);
    let a = r.open_local("p:a").unwrap();
    assert_eq!(r.open_local("p:a").unwrap().id, a.id);
    let first = r.send_local(&a.id, "--help", "request").unwrap();
    assert_eq!(
        r.send_local(&a.id, "--help", "request").unwrap().id,
        first.id
    );
    assert!(matches!(
        r.send_local(&a.id, "changed", "request"),
        Err(Error::IdempotencyConflict)
    ));
    let b = r.open_local("p:b").unwrap();
    assert!(matches!(
        r.send_local(&b.id, "--help", "request"),
        Err(Error::IdempotencyConflict)
    ));
    assert_eq!(count(&conn, "messages"), 1);
}

#[test]
fn removed_agent_history_is_readable_but_new_submissions_fail() {
    let conn = connection();
    let r = Repository::new(&conn);
    assert!(matches!(
        r.open_local("p:missing"),
        Err(Error::UnknownAgent(_))
    ));
    let c = r.open_local("p:a").unwrap();
    r.send_local(&c.id, "first", "request").unwrap();
    conn.execute("DELETE FROM agents WHERE id = 'p:a'", [])
        .unwrap();
    assert_eq!(r.history(&c.id, None, None, 20).unwrap().len(), 1);
    assert!(matches!(
        r.send_local(&c.id, "new", "request-2"),
        Err(Error::UnknownAgent(_))
    ));
}

#[test]
fn local_clients_cannot_impersonate_an_external_human() {
    let conn = connection();
    let r = Repository::new(&conn);
    let remote = r.ingest(inbound("peer", "1")).unwrap();
    assert!(matches!(
        r.send_local(&remote.conversation_id, "fake", "request"),
        Err(Error::Forbidden)
    ));
    let mut reserved = inbound("peer", "2");
    reserved.adapter_id = "local".into();
    assert!(matches!(r.ingest(reserved), Err(Error::Forbidden)));
    assert_eq!(count(&conn, "messages"), 1);
}

#[test]
fn replies_require_owned_routes_and_consistent_references() {
    let conn = connection();
    let r = Repository::new(&conn);
    let a = r.ingest(inbound("peer-a", "1")).unwrap();
    let b = r.ingest(inbound("peer-b", "1")).unwrap();
    assert!(matches!(
        r.reply("p:a", None, None, "reply", "text", None, None),
        Err(Error::AmbiguousRoute)
    ));
    assert!(matches!(
        r.reply(
            "p:b",
            Some(&a.conversation_id),
            None,
            "reply",
            "text",
            None,
            None
        ),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        r.reply("p:b", None, a.mailbox_id, "reply", "text", None, None),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        r.reply(
            "p:a",
            Some(&a.conversation_id),
            b.mailbox_id,
            "reply",
            "text",
            None,
            None
        ),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        r.reply("p:a", None, Some(12345), "reply", "text", None, None),
        Err(Error::NotFound(_))
    ));
    let reply = r
        .reply(
            "p:a",
            Some(&a.conversation_id),
            a.mailbox_id,
            "reply",
            "text",
            None,
            Some("request"),
        )
        .unwrap();
    assert_eq!(reply.status, DeliveryStatus::Queued);
    assert_eq!(count(&conn, "chat_deliveries"), 1);
    assert_eq!(
        r.reply(
            "p:a",
            Some(&a.conversation_id),
            a.mailbox_id,
            "reply",
            "text",
            None,
            Some("request")
        )
        .unwrap()
        .id,
        reply.id
    );
    assert!(matches!(
        r.reply(
            "p:a",
            Some(&a.conversation_id),
            a.mailbox_id,
            "edited",
            "text",
            None,
            Some("request")
        ),
        Err(Error::IdempotencyConflict)
    ));
}

#[test]
fn legacy_reference_has_a_distinguishable_no_route_error() {
    let conn = connection();
    let r = Repository::new(&conn);
    let id = existing(&conn, &inbound("peer", "1"));
    assert!(matches!(
        r.reply("p:a", None, Some(id), "reply", "text", None, None),
        Err(Error::NoRoute)
    ));
    conn.execute("UPDATE messages SET sender = 'p:b' WHERE id = ?1", [id])
        .unwrap();
    assert!(matches!(
        r.reply("p:a", None, Some(id), "reply", "text", None, None),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        r.reply("p:a", None, None, "reply", "text", None, None),
        Err(Error::NoRoute)
    ));
}

#[test]
fn mailbox_metadata_cannot_redirect_an_owned_inbound_to_foreign_route() {
    let conn = connection();
    let r = Repository::new(&conn);
    let a = r.ingest(inbound("peer", "1")).unwrap();
    let b = r.open_local("p:b").unwrap();
    conn.execute(
        "UPDATE messages SET conversation_id = ?1 WHERE id = ?2",
        rusqlite::params![b.id, a.mailbox_id],
    )
    .unwrap();
    assert!(matches!(
        r.reply("p:a", None, a.mailbox_id, "reply", "text", None, None),
        Err(Error::Forbidden)
    ));
}

#[test]
fn pagination_is_chronological_bounded_and_advances_backlogs() {
    let conn = connection();
    let r = Repository::new(&conn);
    let c = r.open_local("p:a").unwrap();
    let mut ids = Vec::new();
    for id in 0..205 {
        ids.push(
            r.send_local(&c.id, &format!("line-{id}"), &id.to_string())
                .unwrap()
                .id,
        );
    }
    let latest = r.history(&c.id, None, None, usize::MAX).unwrap();
    assert_eq!(latest.len(), MAX_PAGE_SIZE);
    assert_eq!(latest[0].id, ids[5]);
    assert_eq!(latest[199].id, ids[204]);
    let after = r.history(&c.id, None, Some(ids[0]), 2).unwrap();
    assert_eq!(
        after.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![ids[1], ids[2]]
    );
    let older = r.history(&c.id, Some(ids[3]), None, 2).unwrap();
    assert_eq!(
        older.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![ids[1], ids[2]]
    );
    assert!(matches!(
        r.history(&c.id, Some(1), Some(2), 2),
        Err(Error::InvalidInput(_))
    ));
    assert!(r.history(&c.id, None, None, 0).is_err());
    assert!(r.list_conversations(0).is_err());
}

#[test]
fn reply_retry_does_not_depend_on_disposable_mailbox_or_current_ambiguity() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = r.ingest(inbound("peer", "1")).unwrap();
    let first = r
        .reply(
            "p:a",
            None,
            input.mailbox_id,
            "reply",
            "text",
            None,
            Some("reply-1"),
        )
        .unwrap();
    conn.execute("DELETE FROM messages", []).unwrap();
    r.open_local("p:a").unwrap();
    assert_eq!(
        r.reply(
            "p:a",
            None,
            input.mailbox_id,
            "reply",
            "text",
            None,
            Some("reply-1")
        )
        .unwrap()
        .id,
        first.id
    );
    assert_eq!(count(&conn, "chat_deliveries"), 1);
    assert!(matches!(
        r.reply(
            "p:a",
            None,
            input.mailbox_id,
            "changed",
            "text",
            None,
            Some("reply-1")
        ),
        Err(Error::IdempotencyConflict)
    ));
}

#[test]
fn local_and_agent_request_namespaces_cannot_collide() {
    let conn = connection();
    conn.execute_batch("INSERT INTO projects(id,name) VALUES ('local','Local'); INSERT INTO agents(id,project_id,role,runtime) VALUES ('local:operator','local','worker','fake');").unwrap();
    let r = Repository::new(&conn);
    let c = r.open_local("local:operator").unwrap();
    let inbound = r.send_local(&c.id, "hello", "same-request").unwrap();
    let reply = r
        .reply(
            "local:operator",
            Some(&c.id),
            inbound.mailbox_id,
            "reply",
            "text",
            None,
            Some("same-request"),
        )
        .unwrap();
    assert_ne!(inbound.id, reply.id);
}

#[test]
fn local_chat_round_trip() {
    let conn = connection();
    let repository = Repository::new(&conn);
    let conversation = repository.open_local("p:a").unwrap();
    let inbound = repository
        .send_local(&conversation.id, "سلام é 📎", "local-1")
        .unwrap();
    let reply = repository
        .reply(
            "p:a",
            None,
            inbound.mailbox_id,
            "received",
            "text",
            None,
            Some("reply-1"),
        )
        .unwrap();
    assert_eq!(reply.status, DeliveryStatus::Sent);
    assert_eq!(
        repository
            .history(&conversation.id, None, None, 10)
            .unwrap(),
        vec![inbound, reply]
    );
}
