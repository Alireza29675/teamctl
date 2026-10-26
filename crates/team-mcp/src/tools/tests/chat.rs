use super::*;
use team_core::chat::{ChatMessage, DeliveryStatus, Direction, InboundMessage, Repository};

fn chat_ctx() -> (Ctx, NamedTempFile) {
    let file = NamedTempFile::new().unwrap();
    let store = Store::open(file.path()).unwrap();
    for agent in ["p:dev", "p:other"] {
        store
            .upsert_agent(agent, "p", "P", "developer", "codex", false)
            .unwrap();
    }
    (Ctx::new("p:dev".into(), store, "t-".into()), file)
}

fn inbound(ctx: &Ctx, adapter: &str, peer: &str, agent: &str, external: &str) -> ChatMessage {
    let conn = ctx.store.conn.lock().unwrap();
    Repository::new(&conn)
        .ingest(InboundMessage {
            adapter_id: adapter.into(),
            peer_id: peer.into(),
            sender_id: format!("human:{peer}"),
            agent_id: agent.into(),
            external_id: external.into(),
            text: format!("hello from {peer}"),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap()
}

fn capabilities(ctx: &Ctx, adapter: &str, kinds: &[&str]) {
    let conn = ctx.store.conn.lock().unwrap();
    Repository::new(&conn)
        .register_capabilities(
            adapter,
            &kinds.iter().map(|kind| (*kind).into()).collect::<Vec<_>>(),
        )
        .unwrap();
}

fn chat_counts(ctx: &Ctx) -> (i64, i64, i64, i64) {
    let conn = ctx.store.conn.lock().unwrap();
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM messages), (SELECT COUNT(*) FROM chat_messages),
                (SELECT COUNT(*) FROM chat_deliveries), (SELECT COUNT(*) FROM chat_requests)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

async fn invoke(ctx: &Ctx, name: &str, arguments: Value) -> Result<Value, String> {
    call(ctx, json!({"name": name, "arguments": arguments}))
        .await
        .map(|response| response["structuredContent"].clone())
}

#[tokio::test]
async fn local_client_round_trip_exposes_metadata_and_keeps_history_after_inbox_gc() {
    let (ctx, file) = chat_ctx();
    // A separate connection models CLI/UI traffic arriving while MCP is open.
    let client = team_core::chat::open(file.path()).unwrap();
    let repository = Repository::new(&client);
    let conversation = repository.open_local("p:dev").unwrap();
    let sent = repository
        .send_local(&conversation.id, "سلام é 📎\nplease help", "local-send")
        .unwrap();
    let mailbox_id = sent.mailbox_id.unwrap();

    let peek = invoke(&ctx, "inbox_peek", json!({})).await.unwrap();
    let message = &peek["messages"][0];
    assert_eq!(message["id"], mailbox_id);
    assert_eq!(message["chat_message_id"], sent.id);
    assert_eq!(message["conversation_id"], conversation.id);
    assert_eq!(message["adapter_id"], "local");
    assert_eq!(message["peer_id"], "operator");
    assert_eq!(message["text"], sent.text);

    let row = ctx.store.inbox_peek("p:dev", 20).unwrap().remove(0);
    for lazy in [false, true] {
        let event = crate::format_channel_event(&row, lazy);
        let meta = &event["params"]["meta"];
        assert_eq!(meta["conversation_id"], conversation.id);
        assert_eq!(meta["chat_message_id"], sent.id.to_string());
        assert_eq!(meta["adapter_id"], "local");
        assert_eq!(meta["peer_id"], "operator");
        assert!(meta.as_object().unwrap().values().all(Value::is_string));
    }

    let read = invoke(&ctx, "inbox_read", json!({"ids": [mailbox_id]}))
        .await
        .unwrap();
    assert_eq!(read["messages"][0]["conversation_id"], conversation.id);
    let reply = invoke(
        &ctx,
        "reply_to_user",
        json!({"reply_to_message_id": mailbox_id, "text": "ready ✅", "request_id": "local-reply"}),
    )
    .await
    .unwrap();
    assert_eq!(reply["recipient"], "human");
    assert_eq!(reply["conversation_id"], conversation.id);
    assert_eq!(
        chat_counts(&ctx).2,
        0,
        "local replies need no delivery daemon"
    );

    client
        .execute("DELETE FROM messages WHERE acked_at IS NOT NULL", [])
        .unwrap();
    drop(client);
    drop(ctx);
    let reopened = team_core::chat::open(file.path()).unwrap();
    let history = Repository::new(&reopened)
        .history(&conversation.id, None, None, 20)
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text, sent.text);
    assert_eq!(history[0].mailbox_id, None);
    assert_eq!(history[1].direction, Direction::Agent);
    assert_eq!(history[1].text, "ready ✅");
    assert_eq!(history[1].status, DeliveryStatus::Sent);
}

#[tokio::test]
async fn two_peers_require_an_address_and_mailbox_reply_keeps_the_exact_peer_and_quote() {
    let (ctx, _file) = chat_ctx();
    let first = inbound(&ctx, "fixture", "alice", "p:dev", "source:a");
    let second = inbound(&ctx, "fixture", "bob", "p:dev", "source:b");
    let row = ctx
        .store
        .inbox_peek("p:dev", 20)
        .unwrap()
        .into_iter()
        .find(|row| row.id == second.mailbox_id.unwrap())
        .unwrap();
    let event = crate::format_channel_event(&row, true);
    assert_eq!(event["params"]["meta"]["external_id"], "source:b");
    assert_eq!(event["params"]["meta"]["peer_id"], "bob");
    assert_eq!(event["params"]["meta"]["adapter_id"], "fixture");
    assert_eq!(
        event["params"]["meta"]["conversation_id"],
        second.conversation_id
    );
    let before = chat_counts(&ctx);
    let error = invoke(&ctx, "reply_to_user", json!({"text": "who?"}))
        .await
        .unwrap_err();
    assert!(error.contains("multiple conversations"), "{error}");
    assert_eq!(chat_counts(&ctx), before);

    let reply = invoke(
        &ctx,
        "reply_to_user",
        json!({
            "reply_to_message_id": second.mailbox_id, "text": "hello Bob"
        }),
    )
    .await
    .unwrap();
    assert_eq!(reply["conversation_id"], second.conversation_id);
    let conn = ctx.store.conn.lock().unwrap();
    let deliveries = Repository::new(&conn)
        .claim_deliveries("fixture", "test", Store::now(), 30.0, 20)
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].peer_id, "bob");
    assert_eq!(
        deliveries[0].reply_to_external_id.as_deref(),
        Some("source:b")
    );
    assert_eq!(
        Repository::new(&conn)
            .history(&first.conversation_id, None, None, 20)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn foreign_or_conflicting_chat_references_fail_without_enqueuing_any_reply() {
    let (ctx, _file) = chat_ctx();
    let own = inbound(&ctx, "fixture", "alice", "p:dev", "a");
    let other_peer = inbound(&ctx, "fixture", "bob", "p:dev", "b");
    let foreign = inbound(&ctx, "fixture", "carol", "p:other", "c");
    let before = chat_counts(&ctx);
    for address in [
        json!({"conversation_id": foreign.conversation_id}),
        json!({"reply_to_message_id": foreign.mailbox_id}),
        json!({"conversation_id": own.conversation_id, "reply_to_message_id": other_peer.mailbox_id}),
        json!({"conversation_id": "chat:missing"}),
        json!({"reply_to_message_id": 999_999}),
    ] {
        let mut arguments = address;
        arguments["text"] = json!("must not escape");
        assert!(invoke(&ctx, "reply_to_user", arguments).await.is_err());
        assert_eq!(chat_counts(&ctx), before);
    }
}

#[tokio::test]
async fn explicit_legacy_inbound_remains_a_telegram_reply_after_local_chat_is_opened() {
    let (ctx, _file) = ctx_with_manager();
    let mailbox_id = seed_inbound_row(&ctx.store, 12345);
    let local_id = {
        let conn = ctx.store.conn.lock().unwrap();
        Repository::new(&conn).open_local("p:mgr").unwrap().id
    };
    let response = invoke(
        &ctx,
        "reply_to_user",
        json!({
            "reply_to_message_id": mailbox_id, "text": "legacy answer"
        }),
    )
    .await
    .unwrap();
    assert_eq!(response["recipient"], "user:telegram");
    assert_eq!(
        fetch_telegram_msg_id(&ctx.store, response["id"].as_i64().unwrap()),
        Some(12345)
    );
    let conn = ctx.store.conn.lock().unwrap();
    assert!(Repository::new(&conn)
        .history(&local_id, None, None, 20)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn multipart_request_retry_is_identical_and_reusing_its_shape_is_rejected() {
    let (ctx, _file) = chat_ctx();
    let source = inbound(&ctx, "fixture", "alice", "p:dev", "a");
    capabilities(&ctx, "fixture", &["text", "image"]);
    let arguments = json!({
        "conversation_id": source.conversation_id,
        "request_id": "report",
        "text": "report ready",
        "image": {"source": "url", "value": "https://example.com/report.png", "caption": "preview"}
    });
    let first = invoke(&ctx, "reply_to_user", arguments.clone())
        .await
        .unwrap();
    assert_eq!(first["ids"].as_array().unwrap().len(), 2);
    let before = chat_counts(&ctx);
    assert_eq!(
        invoke(&ctx, "reply_to_user", arguments.clone())
            .await
            .unwrap(),
        first
    );
    assert_eq!(chat_counts(&ctx), before);
    for changed in [
        json!({"conversation_id": source.conversation_id, "request_id": "report", "text": "report ready"}),
        {
            let mut value = arguments.clone();
            value["image"]["caption"] = json!("different");
            value
        },
        {
            let mut value = arguments.clone();
            value["text"] = json!("changed");
            value
        },
    ] {
        let error = invoke(&ctx, "reply_to_user", changed).await.unwrap_err();
        assert!(error.contains("reused with different content"), "{error}");
        assert_eq!(chat_counts(&ctx), before);
    }
}

#[tokio::test]
async fn unsupported_media_rejects_the_whole_reply_before_a_text_part_is_written() {
    let (ctx, _file) = chat_ctx();
    let source = inbound(&ctx, "fixture", "alice", "p:dev", "a");
    capabilities(&ctx, "fixture", &["text"]);
    let before = chat_counts(&ctx);
    let error = invoke(&ctx, "reply_to_user", json!({
        "conversation_id": source.conversation_id, "request_id": "unsupported",
        "text": "must stay unsent", "image": {"source": "url", "value": "https://example.com/image.png"}
    })).await.unwrap_err();
    assert!(error.contains("does not support image"), "{error}");
    assert_eq!(chat_counts(&ctx), before);
}

#[tokio::test]
async fn local_chat_controls_report_unsupported_without_creating_control_messages() {
    let (ctx, _file) = chat_ctx();
    let source = {
        let conn = ctx.store.conn.lock().unwrap();
        let repo = Repository::new(&conn);
        let conversation = repo.open_local("p:dev").unwrap();
        repo.send_local(&conversation.id, "local hello", "send")
            .unwrap()
    };
    let before = chat_counts(&ctx);
    for (tool, arguments) in [
        (
            "show_typing",
            json!({"conversation_id": source.conversation_id}),
        ),
        (
            "react_to_user",
            json!({"message_id": source.mailbox_id, "emoji": "👍"}),
        ),
    ] {
        let response = invoke(&ctx, tool, arguments).await.unwrap();
        assert_eq!(response["supported"], false);
        assert_eq!(response["conversation_id"], source.conversation_id);
        assert_eq!(chat_counts(&ctx), before);
    }
}

#[tokio::test]
async fn typing_is_scoped_to_the_selected_peer_and_has_a_bounded_expiry() {
    let (ctx, _file) = chat_ctx();
    inbound(&ctx, "fixture", "alice", "p:dev", "a");
    let bob = inbound(&ctx, "fixture", "bob", "p:dev", "b");
    capabilities(&ctx, "fixture", &["text", "typing"]);
    let started = Store::now();
    let response = invoke(
        &ctx,
        "show_typing",
        json!({"reply_to_message_id": bob.mailbox_id}),
    )
    .await
    .unwrap();
    let finished = Store::now();
    assert_eq!(response["supported"], true);
    assert_eq!(response["conversation_id"], bob.conversation_id);
    let conn = ctx.store.conn.lock().unwrap();
    let deliveries = Repository::new(&conn)
        .claim_deliveries("fixture", "test", finished, 30.0, 20)
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].peer_id, "bob");
    assert_eq!(deliveries[0].message.kind, "typing");
    let expires = deliveries[0].message.payload.as_ref().unwrap()["expires_at"]
        .as_f64()
        .unwrap();
    assert!((started + 10.0..=finished + 10.0).contains(&expires));
}

#[tokio::test]
async fn reaction_uses_the_addressed_opaque_external_id_and_peer() {
    let (ctx, _file) = chat_ctx();
    inbound(&ctx, "fixture", "alice", "p:dev", "alice:1");
    let bob = inbound(&ctx, "fixture", "bob", "p:dev", "opaque:message:2");
    capabilities(&ctx, "fixture", &["text", "reaction"]);
    let response = invoke(
        &ctx,
        "react_to_user",
        json!({"message_id": bob.mailbox_id, "emoji": "👍"}),
    )
    .await
    .unwrap();
    assert_eq!(response["supported"], true);
    let conn = ctx.store.conn.lock().unwrap();
    let deliveries = Repository::new(&conn)
        .claim_deliveries("fixture", "test", Store::now(), 30.0, 20)
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].peer_id, "bob");
    assert_eq!(
        deliveries[0].reply_to_external_id.as_deref(),
        Some("opaque:message:2")
    );
    let payload = deliveries[0].message.payload.as_ref().unwrap();
    assert_eq!(payload["external_message_id"], "opaque:message:2");
    assert_eq!(payload["emoji"], "👍");
    assert!(payload.get("telegram_msg_id").is_none());
}

#[tokio::test]
async fn unknown_foreign_and_cross_peer_reactions_never_enqueue_a_control() {
    let (ctx, _file) = chat_ctx();
    let own = inbound(&ctx, "telegram:fixture", "alice", "p:dev", "101");
    let bob = inbound(&ctx, "telegram:fixture", "bob", "p:dev", "202");
    let foreign = inbound(&ctx, "telegram:fixture", "carol", "p:other", "303");
    capabilities(&ctx, "telegram:fixture", &["text", "reaction"]);
    let before = chat_counts(&ctx);
    for address in [
        json!({"message_id": 999_999}),
        json!({"message_id": foreign.mailbox_id}),
        json!({"conversation_id": own.conversation_id, "message_id": bob.mailbox_id}),
        json!({"message_id": own.mailbox_id, "telegram_msg_id": 202}),
        json!({"conversation_id": own.conversation_id, "telegram_msg_id": 202}),
        json!({"conversation_id": own.conversation_id, "telegram_msg_id": 999_999}),
        json!({"conversation_id": foreign.conversation_id, "telegram_msg_id": 303}),
    ] {
        let mut arguments = address;
        arguments["emoji"] = json!("👍");
        assert!(invoke(&ctx, "react_to_user", arguments).await.is_err());
        assert_eq!(chat_counts(&ctx), before);
    }
}

#[tokio::test]
async fn known_telegram_external_reaction_is_scoped_to_the_explicit_conversation() {
    let (ctx, _file) = chat_ctx();
    inbound(&ctx, "telegram:fixture", "alice", "p:dev", "101");
    let bob = inbound(&ctx, "telegram:fixture", "bob", "p:dev", "202");
    capabilities(&ctx, "telegram:fixture", &["text", "reaction"]);
    let response = invoke(
        &ctx,
        "react_to_user",
        json!({"conversation_id": bob.conversation_id, "telegram_msg_id": 202, "emoji": "👍"}),
    )
    .await
    .unwrap();
    assert_eq!(response["supported"], true);
    let conn = ctx.store.conn.lock().unwrap();
    let deliveries = Repository::new(&conn)
        .claim_deliveries("telegram:fixture", "test", Store::now(), 30.0, 20)
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].peer_id, "bob");
    let payload = deliveries[0].message.payload.as_ref().unwrap();
    assert_eq!(payload["telegram_msg_id"], 202);
    assert_eq!(payload["external_message_id"], "202");
}

#[tokio::test]
async fn legacy_typing_accepts_omitted_or_null_arguments() {
    let (ctx, _file) = ctx_with_manager();
    for parameters in [
        json!({"name": "show_typing"}),
        json!({"name": "show_typing", "arguments": null}),
    ] {
        let response = call(&ctx, parameters).await.unwrap();
        let result = &response["structuredContent"];
        assert_eq!(result["recipient"], "user:telegram");
        let (_, kind, _) = fetch_message(&ctx.store, result["id"].as_i64().unwrap());
        assert_eq!(kind.as_deref(), Some("typing"));
    }
}

#[tokio::test]
async fn community_reactions_accept_emoji_outside_telegrams_allowlist() {
    let (ctx, _file) = chat_ctx();
    let source = inbound(&ctx, "fixture", "alice", "p:dev", "opaque:1");
    capabilities(&ctx, "fixture", &["text", "reaction"]);
    let response = invoke(
        &ctx,
        "react_to_user",
        json!({"message_id": source.mailbox_id, "emoji": "🍕"}),
    )
    .await
    .unwrap();
    assert_eq!(response["supported"], true);
    let conn = ctx.store.conn.lock().unwrap();
    let deliveries = Repository::new(&conn)
        .claim_deliveries("fixture", "test", Store::now(), 30.0, 20)
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].message.payload.as_ref().unwrap()["emoji"],
        "🍕"
    );
}

#[tokio::test]
async fn modern_telegram_reactions_keep_the_transport_emoji_restriction() {
    let (ctx, _file) = chat_ctx();
    let source = inbound(&ctx, "telegram:fixture", "alice", "p:dev", "101");
    capabilities(&ctx, "telegram:fixture", &["text", "reaction"]);
    let before = chat_counts(&ctx);
    let error = invoke(
        &ctx,
        "react_to_user",
        json!({"message_id": source.mailbox_id, "emoji": "🍕"}),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("allowlist") && error.contains("🍕"),
        "{error}"
    );
    assert_eq!(chat_counts(&ctx), before);
}

#[tokio::test]
async fn legacy_control_references_stay_legacy_and_resolve_the_addressed_telegram_id() {
    let (ctx, _file) = ctx_with_manager();
    let mailbox = seed_inbound_row(&ctx.store, 12345);
    {
        let conn = ctx.store.conn.lock().unwrap();
        Repository::new(&conn).open_local("p:mgr").unwrap();
    }
    let typing = invoke(&ctx, "show_typing", json!({"reply_to_message_id": mailbox}))
        .await
        .unwrap();
    assert_eq!(typing["recipient"], "user:telegram");
    assert_eq!(
        fetch_telegram_msg_id(&ctx.store, typing["id"].as_i64().unwrap()),
        Some(12345)
    );
    let reaction = invoke(
        &ctx,
        "react_to_user",
        json!({"message_id": mailbox, "emoji": "👍"}),
    )
    .await
    .unwrap();
    assert_eq!(reaction["recipient"], "user:telegram");
    assert_eq!(reaction["telegram_msg_id"], 12345);
    assert_eq!(
        chat_counts(&ctx).1,
        0,
        "legacy controls must not enter the local transcript"
    );
    let before = chat_counts(&ctx);
    assert!(invoke(
        &ctx,
        "react_to_user",
        json!({"message_id": mailbox, "telegram_msg_id": 999, "emoji": "👍"})
    )
    .await
    .is_err());
    assert_eq!(chat_counts(&ctx), before);
}

#[tokio::test]
async fn cached_multipart_reply_survives_deleted_source_files_gc_and_capability_changes() {
    let (ctx, _file) = chat_ctx();
    let source = inbound(&ctx, "fixture", "alice", "p:dev", "a");
    capabilities(&ctx, "fixture", &["text", "image"]);
    let image = NamedTempFile::with_suffix(".png").unwrap();
    std::fs::write(image.path(), b"image fixture").unwrap();
    let arguments = json!({"reply_to_message_id": source.mailbox_id, "request_id": "report",
        "text": "report ready", "image": {"source": "path", "value": image.path()}});
    let first = invoke(&ctx, "reply_to_user", arguments.clone())
        .await
        .unwrap();
    drop(image);
    ctx.store
        .inbox_ack("p:dev", &[source.mailbox_id.unwrap()])
        .unwrap();
    ctx.store
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM messages WHERE acked_at IS NOT NULL", [])
        .unwrap();
    capabilities(&ctx, "fixture", &["text"]);
    let before = chat_counts(&ctx);
    assert_eq!(
        invoke(&ctx, "reply_to_user", arguments).await.unwrap(),
        first
    );
    assert_eq!(chat_counts(&ctx), before);
}

#[tokio::test]
async fn authorization_and_capability_errors_precede_local_file_validation() {
    let (ctx, _file) = chat_ctx();
    let foreign = inbound(&ctx, "fixture", "alice", "p:other", "a");
    let own = inbound(&ctx, "fixture", "bob", "p:dev", "b");
    capabilities(&ctx, "fixture", &["text"]);
    let before = chat_counts(&ctx);
    let foreign_error = invoke(
        &ctx,
        "reply_to_user",
        json!({
            "conversation_id": foreign.conversation_id,
            "image": {"source": "path", "value": "/nonexistent/fixture.png"}
        }),
    )
    .await
    .unwrap_err();
    assert!(foreign_error.contains("does not belong"), "{foreign_error}");
    let unsupported_error = invoke(
        &ctx,
        "reply_to_user",
        json!({
            "conversation_id": own.conversation_id,
            "image": {"source": "path", "value": "/nonexistent/fixture.png"}
        }),
    )
    .await
    .unwrap_err();
    assert!(
        unsupported_error.contains("does not support image"),
        "{unsupported_error}"
    );
    assert_eq!(chat_counts(&ctx), before);
}

#[tokio::test]
async fn named_web_interface_can_be_selected_proactively_and_exposes_source_context() {
    let (ctx, _file) = chat_ctx();
    {
        let conn = ctx.store.conn.lock().unwrap();
        team_core::interfaces::register(
            &conn,
            &team_core::interfaces::InterfaceInfo {
                id: "web".into(),
                provider: "web".into(),
                name: "My workspace".into(),
                description: "Private browser".into(),
                capabilities: vec!["text".into(), "html".into()],
            },
            &["p:dev".into()],
        )
        .unwrap();
    }
    let listed = invoke(&ctx, "list_interfaces", json!({})).await.unwrap();
    assert_eq!(listed["interfaces"][0]["description"], "Private browser");
    let sent=invoke(&ctx,"reply_to_user",json!({"interface_id":"web","text":"Hello","html":"<button>Click</button>","request_id":"proactive"})).await.unwrap();
    assert_eq!(sent["ids"].as_array().unwrap().len(), 2);
    let conversation = sent["conversation_id"].as_str().unwrap();
    let received = {
        let conn = ctx.store.conn.lock().unwrap();
        let repo = Repository::new(&conn);
        let history = repo.history(conversation, None, None, 20).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|m| m.status == DeliveryStatus::Sent));
        repo.send_rich(conversation, "Hi", "text", None, "incoming")
            .unwrap()
    };
    let peek = invoke(&ctx, "inbox_peek", json!({})).await.unwrap();
    assert_eq!(peek["messages"][0]["id"], received.mailbox_id.unwrap());
    assert_eq!(peek["messages"][0]["interface"]["name"], "My workspace");
    assert!(invoke(
        &ctx,
        "reply_to_user",
        json!({"interface_id":"foreign","text":"blocked"})
    )
    .await
    .is_err());
}

#[tokio::test]
async fn web_media_is_cached_under_policy_and_successful_retry_survives_source_deletion() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("team-compose.yaml"),
        "version: '3.0.0'\nprojects: []\n",
    )
    .unwrap();
    let source = root.path().join("picture.png");
    std::fs::write(&source, b"png sample").unwrap();
    let store = Store::open(&root.path().join("state/mailbox.db")).unwrap();
    store
        .upsert_agent("p:dev", "p", "Personal", "dev", "codex", false)
        .unwrap();
    {
        let conn = store.conn.lock().unwrap();
        team_core::interfaces::register(
            &conn,
            &team_core::interfaces::InterfaceInfo {
                id: "web".into(),
                provider: "web".into(),
                name: "Workspace".into(),
                description: "Private".into(),
                capabilities: vec!["text".into(), "image".into()],
            },
            &["p:dev".into()],
        )
        .unwrap();
    }
    let cfg = team_core::compose::Attachments {
        allowed_roots: vec![root.path().to_string_lossy().into_owned()],
        ..Default::default()
    };
    let ctx = Ctx::for_test_with_attachments("p:dev".into(), store, root.path(), cfg, None);
    let args = json!({"interface_id":"web","text":"Screenshot","image":{"source":"path","value":source},"request_id":"asset-send"});
    let first = invoke(&ctx, "reply_to_user", args.clone()).await.unwrap();
    let conversation = first["conversation_id"].as_str().unwrap();
    {
        let conn = ctx.store.conn.lock().unwrap();
        let messages = Repository::new(&conn)
            .history(conversation, None, None, 20)
            .unwrap();
        let asset = &messages[1].payload.as_ref().unwrap()["attachments"][0];
        let id = asset["id"].as_str().unwrap();
        assert_eq!(
            std::fs::read(root.path().join("state/web/assets").join(id)).unwrap(),
            b"png sample"
        );
        assert_eq!(asset["url"], format!("/api/assets/{id}"));
    }
    std::fs::remove_file(source).unwrap();
    assert_eq!(first, invoke(&ctx, "reply_to_user", args).await.unwrap());
    let conn = ctx.store.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM web_assets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn native_compaction_reports_unavailable_instead_of_dispatching_to_tmux() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("team-compose.yaml"),
        "version: '3.0.0'\nprojects: [{file: p.yaml}]\n",
    )
    .unwrap();
    std::fs::write(root.path().join("p.yaml"),"version: 2\nproject: {id: p,name: Test,cwd: .}\nagents:\n  dev: {runtime: codex,execution: {mode: native}}\n").unwrap();
    let store = Store::open(&root.path().join("mailbox.db")).unwrap();
    store
        .upsert_agent("p:dev", "p", "Test", "dev", "codex", false)
        .unwrap();
    let ctx = Ctx::for_test_with_attachments(
        "p:dev".into(),
        store,
        root.path(),
        Default::default(),
        None,
    );
    let error = invoke(&ctx, "compact_self", json!({})).await.unwrap_err();
    assert!(error.contains("unavailable for native execution"));
}
