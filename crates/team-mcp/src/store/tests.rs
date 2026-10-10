use super::*;
use rusqlite::params;
use tempfile::NamedTempFile;

#[test]
fn malformed_acl_and_database_errors_do_not_grant_messaging_permissions() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    s.upsert_agent("p:a", "p", "P", "a", "codex", false)
        .unwrap();
    assert!(s.can_dm("p:a", "p:b").unwrap());
    {
        let conn = s.conn.lock().unwrap();
        conn.execute("INSERT INTO agent_acls (agent_id,can_dm_json,can_bcast_json) VALUES ('p:a','malformed','malformed')", []).unwrap();
    }
    assert!(s.can_dm("p:a", "p:b").is_err());
    assert!(s.can_broadcast("p:a", "all").is_err());
    s.conn
        .lock()
        .unwrap()
        .execute("DROP TABLE agent_acls", [])
        .unwrap();
    assert!(s.can_dm("p:a", "p:b").is_err());
}

#[test]
fn approval_decision_checks_expiry_without_relying_on_the_sweeper() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .request_approval("p", "p:a", "deploy", None, "expired", "{}", 0.0, None)
        .unwrap();
    assert!(!s
        .decide_approval(id, "approved", "user:telegram", None, None)
        .unwrap());
    assert_eq!(s.approval_status(id).unwrap().status, "pending");
}

#[test]
fn approval_delivery_never_marks_expired_or_resolved_rows() {
    let file = NamedTempFile::new().unwrap();
    let store = Store::open(file.path()).unwrap();
    let expired = store
        .request_approval("p", "p:a", "deploy", None, "expired", "{}", 0.0, None)
        .unwrap();
    assert!(!store.mark_delivered(expired).unwrap());
    store.expire_stale_approvals().unwrap();
    assert!(!store.mark_delivered(expired).unwrap());
    let status = store.approval_status(expired).unwrap();
    assert_eq!(status.status, "undeliverable");
    assert!(status.delivered_at.is_none());
    let live = store
        .request_approval("p", "p:a", "deploy", None, "live", "{}", 60.0, None)
        .unwrap();
    assert!(store.mark_delivered(live).unwrap());
    assert!(!store.mark_delivered(live).unwrap());
    let resolved = store
        .request_approval("p", "p:a", "deploy", None, "resolved", "{}", 60.0, None)
        .unwrap();
    assert!(store
        .decide_approval(resolved, "denied", "user:cli", None, None)
        .unwrap());
    assert!(!store.mark_delivered(resolved).unwrap());
    assert!(store
        .approval_status(resolved)
        .unwrap()
        .delivered_at
        .is_none());
}

#[test]
fn acknowledgement_is_scoped_to_the_caller_and_deliverable_rows() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let private = s.send_dm("p", "p:a", "p:b", "secret", None, None).unwrap();
    let pending = s
        .send_dm_kind(
            "p",
            "p:a",
            "p:b",
            "pending",
            None,
            "media_pending",
            "{}",
            None,
        )
        .unwrap();
    assert_eq!(s.inbox_ack("p:c", &[private]).unwrap(), 0);
    assert_eq!(s.inbox_ack("p:b", &[pending]).unwrap(), 0);
    assert!(s.inbox_read("p:b", &[pending]).unwrap().is_empty());
    assert_eq!(s.inbox_peek("p:b", 10).unwrap().len(), 1);
    assert_eq!(s.inbox_depth("p:b").unwrap(), 1);
    assert_eq!(s.inbox_ack("p:b", &[private, private]).unwrap(), 1);
    assert_eq!(s.inbox_ack("p:b", &[private]).unwrap(), 0);
}

#[test]
fn channel_subscribers_consume_independently() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    {
        let c = s.conn.lock().unwrap();
        c.execute("INSERT INTO projects (id, name) VALUES ('p', 'p')", [])
            .unwrap();
        c.execute(
            "INSERT INTO channels (id, project_id, name) VALUES ('p:all', 'p', 'all')",
            [],
        )
        .unwrap();
        for agent in ["p:a", "p:b"] {
            c.execute(
                "INSERT INTO agents (id, project_id, role, runtime) VALUES (?1, 'p', ?1, 'codex')",
                params![agent],
            )
            .unwrap();
            c.execute(
                "INSERT INTO channel_members (channel_id, agent_id) VALUES ('p:all', ?1)",
                params![agent],
            )
            .unwrap();
        }
    }
    let id = s
        .send_broadcast("p", "p:sender", "all", "both should read")
        .unwrap();
    assert_eq!(s.inbox_read("p:a", &[id]).unwrap().len(), 1);
    assert!(s.inbox_peek("p:a", 10).unwrap().is_empty());
    assert_eq!(s.inbox_depth("p:a").unwrap(), 0);
    assert_eq!(s.inbox_peek("p:b", 10).unwrap()[0].id, id);
    assert_eq!(s.inbox_depth("p:b").unwrap(), 1);
    assert_eq!(s.inbox_ack("p:b", &[id]).unwrap(), 1);
    let c = s.conn.lock().unwrap();
    let ack: Option<f64> = c
        .query_row(
            "SELECT acked_at FROM messages WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ack.is_some());
}

#[test]
fn watcher_filters_high_water_before_the_batch_limit() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let mut last = 0;
    for _ in 0..150 {
        last = s.send_dm("p", "p:a", "p:b", "old", None, None).unwrap();
    }
    let fresh = s.send_dm("p", "p:a", "p:b", "fresh", None, None).unwrap();
    assert_eq!(s.inbox_since("p:b", last, 100).unwrap()[0].id, fresh);
}

#[test]
fn roundtrip_dm_and_ack() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .send_dm("hello", "hello:mgr", "hello:dev", "hi", None, None)
        .unwrap();
    let msgs = s.inbox_peek("hello:dev", 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].id, id);
    assert_eq!(s.inbox_ack("hello:dev", &[id]).unwrap(), 1);
    assert!(s.inbox_peek("hello:dev", 10).unwrap().is_empty());
}

#[test]
fn send_dm_kind_persists_kind_and_payload() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let payload = r#"{"source":"path","value":"/tmp/x.png","caption":"hi"}"#;
    let id = s
        .send_dm_kind(
            "p",
            "p:mgr",
            "user:telegram",
            "hi",
            None,
            "image",
            payload,
            None,
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let (kind, structured): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("image"));
    assert_eq!(structured.as_deref(), Some(payload));
}

#[test]
fn send_dm_kind_system_from_system_sender_ok() {
    // #254: a `system:*` source may originate kind='system'.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .send_dm_kind(
            "p",
            "system:supervisor",
            "p:dev",
            "session terminating in 60s, call drained()",
            None,
            "system",
            "{}",
            None,
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM messages WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("system"));
}

#[test]
fn send_dm_kind_system_from_agent_rejected() {
    // #254: an ordinary agent cannot forge a system signal.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let err = s
        .send_dm_kind(
            "p",
            "p:dev",
            "p:mgr",
            "fake drain",
            None,
            "system",
            "{}",
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("system:*"),
        "error must name the system:* restriction, got: {err}"
    );
    // The rejected message must not have been inserted.
    let conn = s.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "rejected system insert must not persist a row");
}

#[test]
fn send_dm_kind_system_from_user_rejected() {
    // #254: `user:*` cannot originate a system signal either.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let err = s
        .send_dm_kind(
            "p",
            "user:telegram",
            "p:dev",
            "fake startup",
            None,
            "system",
            "{}",
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("system:*"), "got: {err}");
}

#[test]
fn send_dm_kind_non_system_kind_unaffected_by_allowlist() {
    // #254: the allowlist fires only for kind='system'. Existing
    // structured kinds from ordinary senders are untouched.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let payload = r#"{"source":"path","value":"/tmp/x.png"}"#;
    s.send_dm_kind(
        "p",
        "p:mgr",
        "user:telegram",
        "pic",
        None,
        "image",
        payload,
        None,
    )
    .expect("non-system kinds must not be gated by the system allowlist");
}

#[test]
fn inbox_peek_surfaces_system_kind() {
    // #254 req 4: inbox tools pass system rows through unchanged.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    s.send_dm_kind(
        "p",
        "system:supervisor",
        "p:dev",
        "you were down for 12 minutes",
        None,
        "system",
        "{}",
        None,
    )
    .unwrap();
    let msgs = s.inbox_peek("p:dev", 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].kind.as_deref(), Some("system"));
    assert_eq!(msgs[0].text, "you were down for 12 minutes");
}

#[test]
fn request_approval_binary_leaves_options_json_null() {
    // Back-compat: a binary request (no options) must leave
    // options_json NULL so the bot takes the Approve/Deny path.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .request_approval("p", "p:dev", "deploy", None, "ship?", "{}", 900.0, None)
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let opts: Option<String> = conn
        .query_row(
            "SELECT options_json FROM approvals WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(opts.is_none(), "binary request must not set options_json");
}

#[test]
fn request_approval_persists_options_json() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let opts = r#"[{"label":"A","value":"a"},{"label":"B","value":"b"}]"#;
    let id = s
        .request_approval(
            "p",
            "p:dev",
            "decide",
            None,
            "pick one",
            "{}",
            900.0,
            Some(opts),
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: Option<String> = conn
        .query_row(
            "SELECT options_json FROM approvals WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some(opts));
}

#[test]
fn decide_approval_records_chosen_value_and_is_idempotent() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let opts = r#"[{"label":"B","value":"opt_b"}]"#;
    let id = s
        .request_approval("p", "p:dev", "decide", None, "?", "{}", 900.0, Some(opts))
        .unwrap();

    // First decision wins and is recorded with the chosen value.
    let decided = s
        .decide_approval(id, "decided", "user:telegram", Some("opt_b"), None)
        .unwrap();
    assert!(decided, "first decision must report it performed the write");
    let st = s.approval_status(id).unwrap();
    assert_eq!(st.status, "decided");
    assert_eq!(st.value.as_deref(), Some("opt_b"));

    // A second tap against the now-terminal row is a no-op (stale).
    let again = s
        .decide_approval(id, "decided", "user:telegram", Some("other"), None)
        .unwrap();
    assert!(!again, "stale re-decide must report false");
    let value2 = s.approval_status(id).unwrap().value;
    assert_eq!(
        value2.as_deref(),
        Some("opt_b"),
        "value must not be overwritten"
    );
}

#[test]
fn decide_approval_binary_path_leaves_value_null() {
    // The binary Approve/Deny outcome carries the result in `status`;
    // decision_value stays NULL (Cancel takes the same shape).
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .request_approval("p", "p:dev", "deploy", None, "ship?", "{}", 900.0, None)
        .unwrap();
    assert!(s
        .decide_approval(id, "approved", "user:telegram", None, None)
        .unwrap());
    let st = s.approval_status(id).unwrap();
    assert_eq!(st.status, "approved");
    assert!(st.value.is_none());
}

#[test]
fn inbox_peek_surfaces_kind_and_payload_for_structured_rows() {
    // T-086-C contract: inbox_peek must surface the new columns so
    // an agent inspecting a structured row sees the path/caption it
    // needs to read bytes from disk. Without this, inbound media
    // would land in the database but be invisible to the agent.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let payload = r#"{"path":"/tmp/in/7.jpg","mime":"image/jpeg","size_bytes":1024}"#;
    s.send_dm_kind(
        "p",
        "user:telegram",
        "p:mgr",
        "look at this",
        None,
        "image",
        payload,
        None,
    )
    .unwrap();
    let msgs = s.inbox_peek("p:mgr", 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].kind.as_deref(), Some("image"));
    assert_eq!(msgs[0].structured_payload.as_deref(), Some(payload));
    assert_eq!(msgs[0].text, "look at this", "caption mirrors into text");
}

#[test]
fn inbox_peek_returns_null_kind_for_legacy_text_rows() {
    // Back-compat round-trip: a row inserted via the legacy
    // `send_dm` path comes back with `kind = None` and no payload,
    // which the agent's JSON parser will treat as a plain text
    // message exactly as before T-086-A.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    s.send_dm("p", "user:telegram", "p:mgr", "plain text", None, None)
        .unwrap();
    let msgs = s.inbox_peek("p:mgr", 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].kind.is_none());
    assert!(msgs[0].structured_payload.is_none());
}

#[test]
fn legacy_send_dm_leaves_kind_and_payload_null() {
    // Back-compat pin: existing text-only callers route through `send_dm`
    // unchanged. Both new columns must be NULL so readers that treat
    // NULL as 'text' (the bot dispatch path) don't accidentally route a
    // text row as media.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .send_dm("p", "p:mgr", "user:telegram", "hello", None, None)
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let (kind, structured): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(kind.is_none(), "legacy send_dm must leave kind NULL");
    assert!(
        structured.is_none(),
        "legacy send_dm must leave structured_payload NULL"
    );
}

/// Hand-insert an inbound row with a known `telegram_msg_id` and
/// return its mailbox id. Mirrors what team-bot's inbound capture
/// writes when an operator's Telegram message arrives.
fn seed_inbound_row(s: &Store, telegram_msg_id: Option<i64>) -> i64 {
    let conn = s.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, ?1)",
        params![telegram_msg_id],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn send_dm_resolves_mailbox_id_to_telegram_msg_id() {
    // T-168 happy path: agent passes the inbound row's mailbox id
    // (the value visible in the channel envelope as `meta.id`).
    // store.rs resolves it to the row's `telegram_msg_id` at insert
    // time and persists THAT on the outgoing row — what the bot's
    // outbound dispatcher actually feeds `reply_parameters` with.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let inbound_mailbox_id = seed_inbound_row(&s, Some(12345));
    let outbound_id = s
        .send_dm(
            "p",
            "p:mgr",
            "user:telegram",
            "ack",
            None,
            Some(inbound_mailbox_id),
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT telegram_msg_id FROM messages WHERE id = ?1",
            params![outbound_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(12345));
}

#[test]
fn send_dm_leaves_telegram_msg_id_null_when_omitted() {
    // Back-compat pin: callers that don't pass a reply target leave
    // the column NULL so the bot's dispatcher omits
    // `reply_parameters` and the message lands as a fresh post.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let id = s
        .send_dm("p", "p:mgr", "user:telegram", "ack", None, None)
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT telegram_msg_id FROM messages WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.is_none());
}

#[test]
fn send_dm_resolution_misses_when_row_has_no_telegram_msg_id() {
    // T-168 miss case A: agent references an inbound row that
    // exists but isn't a Telegram-sourced row (telegram_msg_id is
    // NULL on peer-to-peer or system rows). Resolution returns
    // None; the outgoing row sends without threading rather than
    // failing the whole call.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let inbound_mailbox_id = seed_inbound_row(&s, None);
    let outbound_id = s
        .send_dm(
            "p",
            "p:mgr",
            "user:telegram",
            "ack",
            None,
            Some(inbound_mailbox_id),
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT telegram_msg_id FROM messages WHERE id = ?1",
            params![outbound_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.is_none());
}

#[test]
fn send_dm_resolution_misses_when_mailbox_id_does_not_exist() {
    // T-168 miss case B: agent passes a mailbox id that points at
    // no row at all (stale id, off-by-one, etc.). Same posture:
    // resolution misses, the send still goes through unthreaded.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let outbound_id = s
        .send_dm("p", "p:mgr", "user:telegram", "ack", None, Some(999_999))
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT telegram_msg_id FROM messages WHERE id = ?1",
            params![outbound_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.is_none());
}

#[test]
fn send_dm_kind_resolves_mailbox_id_to_telegram_msg_id() {
    // T-168 happy path for media: image attached as a reply
    // resolves the mailbox id the same way `send_dm` does. Kind +
    // payload + resolved telegram_msg_id all persist on one row.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    let inbound_mailbox_id = seed_inbound_row(&s, Some(99));
    let outbound_id = s
        .send_dm_kind(
            "p",
            "p:mgr",
            "user:telegram",
            "screenshot",
            None,
            "image",
            r#"{"source":"url","value":"https://x.test/a.png"}"#,
            Some(inbound_mailbox_id),
        )
        .unwrap();
    let conn = s.conn.lock().unwrap();
    let (kind, telegram_msg_id): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT kind, telegram_msg_id FROM messages WHERE id = ?1",
            params![outbound_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("image"));
    assert_eq!(telegram_msg_id, Some(99));
}

#[test]
fn inbox_peek_surfaces_telegram_msg_id_field() {
    // Agent-side: the field on the `Message` struct is what the MCP
    // tool surfaces back to the caller, so the agent can read which
    // Telegram id the operator's message had and thread its reply.
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    // Hand-insert an inbound row with telegram_msg_id set, mimicking
    // what team-bot's inbound capture would write.
    {
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p','P')
             ON CONFLICT(id) DO NOTHING",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agents (id, project_id, role, runtime, is_manager, reports_to)
             VALUES ('p:mgr','p','manager','claude-code',1,NULL)
             ON CONFLICT(id) DO NOTHING",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, telegram_msg_id)
             VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, 4242)",
            [],
        )
        .unwrap();
    }
    let msgs = s.inbox_peek("p:mgr", 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].telegram_msg_id,
        Some(4242),
        "inbox_peek must expose the captured Telegram id to the agent"
    );
}

#[test]
fn upsert_agent_is_idempotent() {
    let f = NamedTempFile::new().unwrap();
    let s = Store::open(f.path()).unwrap();
    s.upsert_agent(
        "hello:mgr",
        "hello",
        "Hello",
        "product-mgr",
        "claude-code",
        true,
    )
    .unwrap();
    s.upsert_agent(
        "hello:mgr",
        "hello",
        "Hello",
        "product-mgr",
        "claude-code",
        true,
    )
    .unwrap();
    s.upsert_agent("hello:dev", "hello", "Hello", "dev", "claude-code", false)
        .unwrap();
    assert_eq!(
        s.list_project_agents("hello").unwrap(),
        vec!["hello:dev".to_string(), "hello:mgr".into()]
    );
}
