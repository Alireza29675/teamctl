use super::*;
use crate::chat::{inbound_message, record_text};
use team_core::chat::{DeliveryStatus, Repository};

fn message(chat: i64, id: i32, text: &str) -> Message {
    let mut msg = inbound(text);
    msg.chat.id = ChatId(chat);
    msg.id = MessageId(id);
    msg
}

fn anonymous_message(id: i32, proxy: bool, content: Value) -> Message {
    let mut value = json!({
        "message_id": id, "date": 0,
        "chat": {"id": -1007, "type": "supergroup", "title": "Authorized group"},
        "sender_chat": {"id": -100700, "type": "supergroup", "title": "Anonymous sender"}
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(content.as_object().unwrap().clone());
    if proxy {
        value["from"] = json!({
            "id": 1087968824_u64, "is_bot": true,
            "first_name": "Group", "username": "GroupAnonymousBot"
        });
    }
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn authorized_anonymous_text_uses_sender_chat_with_or_without_a_proxy() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(-1007);
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![true; 2]).await;
    for proxy in [true, false] {
        let msg = anonymous_message(21, proxy, json!({"text":"anonymous request"}));
        let source =
            inbound_message(&state, &msg, "p:manager", "anonymous request", "text", None).unwrap();
        assert_eq!(source.sender_id, "chat:-100700");
        assert_eq!(source.peer_id, "-1007");
        let mut bot_echo = msg.clone();
        bot_echo.sender_chat = None;
        assert!(inbound_message(
            &state,
            &bot_echo,
            "p:manager",
            "anonymous request",
            "text",
            None
        )
        .is_err());
        let mut foreign_peer = msg.clone();
        foreign_peer.chat.id = ChatId(-1008);
        assert!(inbound_message(
            &state,
            &foreign_peer,
            "p:manager",
            "anonymous request",
            "text",
            None
        )
        .is_err());
        handle_message(bot.clone(), msg, state.clone())
            .await
            .unwrap();
    }
    let requests = server.await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request["text"] == "→ p:manager"));
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().pop().unwrap();
    assert_eq!(conversation.peer_id, "-1007");
    let history = repo.history(&conversation.id, None, None, 10).unwrap();
    assert_eq!(
        history.len(),
        1,
        "the fake proxy must not affect source deduplication"
    );
    assert_eq!(history[0].text, "anonymous request");
    assert_eq!(history[0].external_id.as_deref(), Some("21"));
    let (sender, kind): (String, String) = conn
        .query_row(
            "SELECT sender,kind FROM messages WHERE id=?1",
            [history[0].mailbox_id.unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(sender, "user:telegram");
    assert_eq!(kind, "text");
}

#[tokio::test]
async fn authorized_anonymous_media_errors_publish_with_or_without_a_proxy() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(-1007);
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![false, true, false, true]).await;
    for (id, proxy) in [(31, true), (32, false)] {
        let msg = anonymous_message(
            id,
            proxy,
            json!({
                "caption":"anonymous photo",
                "photo":[{"file_id":"photo", "file_unique_id":"unique", "width":10, "height":10, "file_size":12}]
            }),
        );
        let source =
            inbound_message(&state, &msg, "p:manager", "anonymous photo", "media", None).unwrap();
        assert_eq!(source.sender_id, "chat:-100700");
        handle_message(bot.clone(), msg, state.clone())
            .await
            .unwrap();
    }
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 4);
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().pop().unwrap();
    assert_eq!(conversation.peer_id, "-1007");
    let history = repo.history(&conversation.id, None, None, 10).unwrap();
    assert_eq!(history.len(), 2);
    for (message, external_id) in history.iter().zip(["31", "32"]) {
        assert_eq!(message.kind, "media_error");
        assert_eq!(message.external_id.as_deref(), Some(external_id));
        let sender: String = conn
            .query_row(
                "SELECT sender FROM messages WHERE id=?1",
                [message.mailbox_id.unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sender, "user:telegram");
    }
}

#[tokio::test]
async fn anonymous_voice_fixture_keeps_the_sender_chat_identity_when_publishing_a_transcript() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(-1007);
    let decision = map_voice_outcome(&SttOutcome::Ok("voice request".into()));
    let transcript = decision.inbox_text.unwrap();
    for proxy in [true, false] {
        let msg = anonymous_message(
            41,
            proxy,
            json!({
                "voice":{"file_id":"voice", "file_unique_id":"unique", "duration":1, "mime_type":"audio/ogg", "file_size":12}
            }),
        );
        assert!(msg.voice().is_some());
        let mut source = inbound_message(&state, &msg, "p:manager", "", "voice", None).unwrap();
        assert_eq!(source.sender_id, "chat:-100700");
        source.text = transcript.clone();
        source.kind = "text".into();
        let conn = state.conn.lock().await;
        record_text(&conn, source, 41, None).unwrap();
    }
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().pop().unwrap();
    let history = repo.history(&conversation.id, None, None, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, transcript);
    assert_eq!(history[0].external_id.as_deref(), Some("41"));
    assert_eq!(history[0].kind, "text");
}

#[tokio::test]
async fn two_chats_share_source_ids_without_sharing_conversations_or_reply_targets() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(8);
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![true; 5]).await;
    for (chat, text) in [(7, "first human"), (8, "second human"), (7, "first human")] {
        handle_message(bot.clone(), message(chat, 21, text), state.clone())
            .await
            .unwrap();
    }
    let sources = {
        let conn = state.conn.lock().await;
        let repo = Repository::new(&conn);
        let conversations = repo.list_conversations(10).unwrap();
        assert_eq!(conversations.len(), 2);
        let mut sources = Vec::new();
        for conversation in conversations {
            let history = repo.history(&conversation.id, None, None, 10).unwrap();
            assert_eq!(history.len(), 1, "source replay must not duplicate text");
            assert_eq!(history[0].external_id.as_deref(), Some("21"));
            let source = history[0].mailbox_id.unwrap();
            assert_eq!(
                lookup_replied_row(&conn, &state.adapter_id(), &conversation.peer_id, 21)
                    .unwrap()
                    .unwrap()
                    .0,
                Some(source)
            );
            repo.reply(
                "p:manager",
                Some(&conversation.id),
                Some(source),
                &format!("reply for {}", conversation.peer_id),
                "text",
                None,
                None,
            )
            .unwrap();
            sources.push((conversation.peer_id, source));
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 2,
            "the compatibility replay row is discarded atomically"
        );
        sources
    };
    chat_outbound_once(&bot, &state).await.unwrap();
    chat_outbound_once(&bot, &state).await.unwrap();
    let requests = server.await.unwrap();
    let replies = &requests[3..];
    assert_eq!(replies.len(), 2);
    for request in replies {
        let peer = request["chat_id"].as_i64().unwrap().to_string();
        assert!(request["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("reply for {peer}")));
        assert_eq!(request["reply_parameters"]["message_id"], 21);
    }
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    for (peer, _) in sources {
        let conversation = repo
            .list_conversations(10)
            .unwrap()
            .into_iter()
            .find(|conversation| conversation.peer_id == peer)
            .unwrap();
        let history = repo.history(&conversation.id, None, None, 10).unwrap();
        assert_eq!(history[1].status, DeliveryStatus::Sent);
        assert_eq!(history[1].external_id.as_deref(), Some("1"));
        assert_eq!(
            lookup_replied_row(&conn, &state.adapter_id(), &peer, 1).unwrap(),
            Some((None, "p:manager".into()))
        );
    }
}

#[tokio::test]
async fn explicit_dm_preserves_operator_routing_and_reply_returns_to_its_source_chat() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(8);
    {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO agents(id,project_id,role,runtime,is_manager)
                       VALUES ('p:other','p','other','codex',1)",
            [],
        )
        .unwrap();
    }
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![true; 2]).await;
    handle_message(
        bot.clone(),
        message(8, 21, "/dm p:other /readnow hello"),
        state.clone(),
    )
    .await
    .unwrap();
    {
        let conn = state.conn.lock().await;
        let repo = Repository::new(&conn);
        let conversation = repo.list_conversations(10).unwrap().pop().unwrap();
        assert_eq!(conversation.agent_id, "p:other");
        let source = repo.history(&conversation.id, None, None, 10).unwrap()[0]
            .mailbox_id
            .unwrap();
        let mode: String = conn
            .query_row(
                "SELECT delivery_mode FROM messages WHERE id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mode, "immediate");
        assert!(matches!(
            repo.reply(
                "p:manager",
                Some(&conversation.id),
                None,
                "forged",
                "text",
                None,
                None
            ),
            Err(team_core::chat::Error::Forbidden)
        ));
        repo.reply(
            "p:other",
            None,
            Some(source),
            "hello back",
            "text",
            None,
            None,
        )
        .unwrap();
    }
    chat_outbound_once(&bot, &state).await.unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests[1]["chat_id"], 8);
    assert!(requests[1]["text"]
        .as_str()
        .unwrap()
        .ends_with("— replied by p:other"));
}

#[tokio::test]
async fn forbidden_peer_and_unsupported_content_finish_with_an_actionable_error() {
    let dir = TempDir::new().unwrap();
    let state = state_at(&dir.path().join("mailbox.sqlite"));
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    let mut source = inbound_message(
        &state,
        &message(7, 21, "hello"),
        "p:manager",
        "hello",
        "text",
        None,
    )
    .unwrap();
    source.peer_id = "8".into();
    let foreign_peer = repo.ingest(source).unwrap();
    repo.reply(
        "p:manager",
        Some(&foreign_peer.conversation_id),
        None,
        "blocked",
        "text",
        None,
        None,
    )
    .unwrap();
    let source = inbound_message(
        &state,
        &message(7, 22, "voice please"),
        "p:manager",
        "voice please",
        "text",
        None,
    )
    .unwrap();
    let supported_peer = record_text(&conn, source, 22, None).unwrap();
    repo.reply(
        "p:manager",
        Some(&supported_peer.conversation_id),
        None,
        "",
        "audio",
        Some(json!({"source":"url","value":"https://example.invalid/audio"})),
        None,
    )
    .unwrap();
    drop(conn);
    // No API substitute is needed: both deliveries are rejected before any send.
    let bot = Bot::new("123:test-token").set_api_url("http://127.0.0.1:1/".parse().unwrap());
    chat_outbound_once(&bot, &state).await.unwrap();
    chat_outbound_once(&bot, &state).await.unwrap();
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    for conversation in repo.list_conversations(10).unwrap() {
        let history = repo.history(&conversation.id, None, None, 10).unwrap();
        assert_eq!(history[1].status, DeliveryStatus::Failed);
        assert!(history[1].error.as_deref().is_some_and(
            |error| error.contains("authorized") || error.contains("does not support")
        ));
    }
    let unfinished: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_deliveries WHERE finished=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unfinished, 0);
}

#[tokio::test]
async fn generic_failures_survive_restart_and_do_not_consume_legacy_fallback_rows() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mailbox.sqlite");
    let state = state_at(&path);
    let conversation = {
        let conn = state.conn.lock().await;
        let source = inbound_message(
            &state,
            &inbound("hello"),
            "p:manager",
            "hello",
            "text",
            None,
        )
        .unwrap();
        let source = record_text(&conn, source, 21, None).unwrap();
        Repository::new(&conn)
            .reply(
                "p:manager",
                None,
                source.mailbox_id,
                "generic",
                "text",
                None,
                None,
            )
            .unwrap();
        conn.execute(
            "INSERT INTO messages(project_id,sender,recipient,text,sent_at)
                      VALUES ('p','p:manager','user:telegram','legacy',0)",
            [],
        )
        .unwrap();
        source.conversation_id
    };
    let (bot, server) = fake_bot(vec![false, true, true]).await;
    assert!(chat_outbound_once(&bot, &state).await.is_err());
    {
        let conn = state.conn.lock().await;
        let history = Repository::new(&conn)
            .history(&conversation, None, None, 10)
            .unwrap();
        assert_eq!(history[1].status, DeliveryStatus::Failed);
        assert_eq!(outgoing_messages(&conn, Some("p")).unwrap().len(), 1);
        // Advance the durable backoff without slowing this offline fixture.
        conn.execute("UPDATE chat_deliveries SET next_attempt_at=0", [])
            .unwrap();
    }
    drop(state);
    let restarted = state_at(&path);
    outbound_once(&bot, &restarted, ChatId(7)).await.unwrap();
    outbound_once(&bot, &restarted, ChatId(7)).await.unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    let conn = restarted.conn.lock().await;
    let history = Repository::new(&conn)
        .history(&conversation, None, None, 10)
        .unwrap();
    assert_eq!(history[1].status, DeliveryStatus::Sent);
    assert_eq!(history[1].external_id.as_deref(), Some("1"));
    assert!(outgoing_messages(&conn, Some("p")).unwrap().is_empty());
}

#[tokio::test]
async fn chat_bridge_failure_rolls_back_media_publication_and_keeps_staging_hidden() {
    let dir = TempDir::new().unwrap();
    let state = state_at(&dir.path().join("mailbox.sqlite"));
    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT INTO messages(project_id,sender,recipient,text,sent_at,kind,telegram_msg_id)
                  VALUES ('p','user:telegram','p:manager','caption',0,'media_pending',21)",
        [],
    )
    .unwrap();
    let staged = conn.last_insert_rowid();
    conn.execute_batch("CREATE TRIGGER fail_chat BEFORE INSERT ON chat_sources BEGIN SELECT RAISE(FAIL,'test chat failure'); END;").unwrap();
    let source = inbound_message(
        &state,
        &inbound("caption"),
        "p:manager",
        "caption",
        "media",
        None,
    )
    .unwrap();
    assert!(finalize_media(
        &conn,
        staged,
        "image",
        "{\"path\":\"/tmp/media.png\"}",
        source.clone()
    )
    .is_err());
    let (count, kind): (i64, String) = conn
        .query_row("SELECT COUNT(*),kind FROM messages", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(kind, "media_pending");
    assert!(Repository::new(&conn)
        .list_conversations(10)
        .unwrap()
        .is_empty());
    conn.execute_batch("DROP TRIGGER fail_chat").unwrap();
    let published = finalize_media(
        &conn,
        staged,
        "media_error",
        "{\"error\":\"disk full\"}",
        source,
    )
    .unwrap();
    let conversation = Repository::new(&conn)
        .list_conversations(10)
        .unwrap()
        .pop()
        .unwrap();
    let history = Repository::new(&conn)
        .history(&conversation.id, None, None, 10)
        .unwrap();
    assert_eq!(history[0].mailbox_id, Some(published));
    assert_eq!(history[0].kind, "media_error");
    assert_eq!(history[0].external_id.as_deref(), Some("21"));
}

#[tokio::test]
async fn malformed_or_unknown_ingress_never_publishes_a_partial_row() {
    let dir = TempDir::new().unwrap();
    let state = state_at(&dir.path().join("mailbox.sqlite"));
    let mut msg = inbound("hello");
    msg.from = None;
    assert!(inbound_message(&state, &msg, "p:manager", "hello", "text", None).is_err());
    assert!(inbound_message(
        &state,
        &message(8, 21, "hello"),
        "p:manager",
        "hello",
        "text",
        None
    )
    .is_err());
    let conn = state.conn.lock().await;
    let source = inbound_message(
        &state,
        &inbound("hello"),
        "p:unknown",
        "hello",
        "text",
        None,
    )
    .unwrap();
    assert!(record_text(&conn, source, 21, None).is_err());
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn actual_media_error_publication_is_visible_once_with_its_original_peer_and_source() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state_at(&dir.path().join("mailbox.sqlite")));
    let msg: Message = serde_json::from_value(json!({
        "message_id": 31, "date": 0, "chat": {"id": 7, "type": "private"},
        "from": {"id": 17, "is_bot": false, "first_name": "Operator"},
        "caption": "photo caption", "photo": [{"file_id": "photo", "file_unique_id": "unique",
            "width": 10, "height": 10, "file_size": 12}]
    }))
    .unwrap();
    let (bot, server) = fake_bot(vec![false, true, true]).await;
    handle_message(bot.clone(), msg.clone(), state.clone())
        .await
        .unwrap();
    handle_message(bot, msg, state.clone()).await.unwrap();
    let requests = server.await.unwrap();
    assert_eq!(
        requests.len(),
        3,
        "the replay sends confirmation without another download"
    );
    let conn = state.conn.lock().await;
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().pop().unwrap();
    assert_eq!(conversation.peer_id, "7");
    let history = repo.history(&conversation.id, None, None, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].kind, "media_error");
    assert_eq!(history[0].external_id.as_deref(), Some("31"));
    assert_eq!(history[0].text, "photo caption");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn another_adapter_and_an_active_lease_cannot_be_consumed_by_this_bot() {
    let dir = TempDir::new().unwrap();
    let state = state_at(&dir.path().join("mailbox.sqlite"));
    {
        let conn = state.conn.lock().await;
        let repo = Repository::new(&conn);
        let source = inbound_message(
            &state,
            &inbound("hello"),
            "p:manager",
            "hello",
            "text",
            None,
        )
        .unwrap();
        let inbound = record_text(&conn, source.clone(), 21, None).unwrap();
        repo.reply(
            "p:manager",
            Some(&inbound.conversation_id),
            None,
            "ours",
            "text",
            None,
            None,
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        repo.claim_deliveries(&state.adapter_id(), "another-worker", now, 120.0, 1)
            .unwrap();
        let mut source = source;
        source.adapter_id = "telegram:p:other".into();
        let other = repo.ingest(source).unwrap();
        repo.reply(
            "p:manager",
            Some(&other.conversation_id),
            None,
            "other adapter",
            "text",
            None,
            None,
        )
        .unwrap();
    }
    let (bot, server) = fake_bot(vec![true]).await;
    chat_outbound_once(&bot, &state).await.unwrap();
    {
        let conn = state.conn.lock().await;
        conn.execute(
            "UPDATE chat_deliveries SET lease_until=0 WHERE adapter_id=?1",
            [state.adapter_id()],
        )
        .unwrap();
    }
    chat_outbound_once(&bot, &state).await.unwrap();
    let requests = server.await.unwrap();
    assert!(requests[0]["text"].as_str().unwrap().starts_with("ours"));
    let conn = state.conn.lock().await;
    let unfinished: String = conn
        .query_row(
            "SELECT adapter_id FROM chat_deliveries WHERE finished=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unfinished, "telegram:p:other");
}

#[tokio::test]
async fn native_ingress_declares_its_capabilities_for_scoped_and_legacy_instances() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    for manager in [Some("p:manager".into()), None] {
        state.manager = manager;
        let conn = state.conn.lock().await;
        let source = inbound_message(
            &state,
            &inbound("hello"),
            "p:manager",
            "hello",
            "text",
            None,
        )
        .unwrap();
        record_text(&conn, source, 21, None).unwrap();
        assert_eq!(
            Repository::new(&conn)
                .capabilities(&state.adapter_id())
                .unwrap(),
            Some(
                ["text", "image", "file", "typing", "reaction"]
                    .map(str::to_string)
                    .to_vec()
            )
        );
    }
}

#[tokio::test]
async fn generic_typing_uses_its_peer_and_expiry_while_reactions_retain_the_native_payload() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.push(8);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let conversation = {
        let conn = state.conn.lock().await;
        let source = inbound_message(
            &state,
            &message(8, 21, "hello"),
            "p:manager",
            "hello",
            "text",
            None,
        )
        .unwrap();
        let source = record_text(&conn, source, 21, None).unwrap();
        let repo = Repository::new(&conn);
        repo.reply(
            "p:manager",
            Some(&source.conversation_id),
            None,
            "",
            "typing",
            Some(json!({"expires_at":clock-1.0})),
            None,
        )
        .unwrap();
        repo.reply(
            "p:manager",
            Some(&source.conversation_id),
            None,
            "",
            "typing",
            Some(json!({})),
            None,
        )
        .unwrap();
        repo.reply(
            "p:manager",
            Some(&source.conversation_id),
            None,
            "",
            "typing",
            Some(json!({"expires_at":clock+10.0})),
            None,
        )
        .unwrap();
        repo.reply(
            "p:manager",
            Some(&source.conversation_id),
            None,
            "",
            "reaction",
            Some(json!({"telegram_msg_id":21,"emoji":"👀"})),
            None,
        )
        .unwrap();
        source.conversation_id
    };
    let (bot, server) = fake_bot(vec![true; 2]).await;
    for _ in 0..4 {
        chat_outbound_once(&bot, &state).await.unwrap();
    }
    let requests = server.await.unwrap();
    assert_eq!(requests[0]["chat_id"], 8);
    assert_eq!(requests[0]["action"], "typing");
    assert_eq!(requests[1]["chat_id"], 8);
    assert_eq!(requests[1]["message_id"], 21);
    assert_eq!(requests[1]["reaction"][0]["emoji"], "👀");
    let typing = state.typing.lock().await;
    let deadline = typing.get(&ChatId(8)).unwrap();
    assert!(deadline.saturating_duration_since(Instant::now()) <= TYPING_WINDOW_CEILING);
    assert!(!typing.contains_key(&ChatId(7)));
    drop(typing);
    let conn = state.conn.lock().await;
    let history = Repository::new(&conn)
        .history(&conversation, None, None, 10)
        .unwrap();
    assert_eq!(history[1].status, DeliveryStatus::Failed);
    assert!(history[1].error.as_deref().unwrap().contains("expired"));
    assert_eq!(history[2].status, DeliveryStatus::Failed);
    assert!(history[2].error.as_deref().unwrap().contains("expires_at"));
    assert_eq!(history[3].status, DeliveryStatus::Sent);
    assert_eq!(history[4].status, DeliveryStatus::Sent);
}

#[tokio::test]
async fn two_named_bots_for_same_agent_keep_routes_and_inbound_provenance_separate() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mailbox.sqlite");
    let mut personal = state_at(&path);
    personal.interface_id = Some("personal-bot".into());
    let mut work = state_at(&path);
    work.interface_id = Some("work-bot".into());
    {
        let conn = personal.conn.lock().await;
        for (id, name, description) in [
            ("personal-bot", "Personal bot", "Private errands"),
            ("work-bot", "Work bot", "Work requests"),
        ] {
            team_core::interfaces::register(
                &conn,
                &team_core::interfaces::InterfaceInfo {
                    id: id.into(),
                    provider: "telegram".into(),
                    name: name.into(),
                    description: description.into(),
                    capabilities: vec!["text".into()],
                },
                &["p:manager".into()],
            )
            .unwrap();
        }
        for state in [&personal, &work] {
            let inbound = inbound_message(
                state,
                &message(7, 1, "hello"),
                "p:manager",
                "hello",
                "text",
                None,
            )
            .unwrap();
            let received = record_text(&conn, inbound, 1, None).unwrap();
            Repository::new(&conn)
                .reply(
                    "p:manager",
                    Some(&received.conversation_id),
                    received.mailbox_id,
                    state.interface_id.as_deref().unwrap(),
                    "text",
                    None,
                    None,
                )
                .unwrap();
            let provenance: String = conn
                .query_row(
                    "SELECT interface_json FROM messages WHERE id=?1",
                    [received.mailbox_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&provenance).unwrap()["id"],
                state.interface_id.as_deref().unwrap()
            );
        }
        assert_eq!(
            Repository::new(&conn).list_conversations(20).unwrap().len(),
            2
        );
    }
    let (bot, server) = fake_bot(vec![true; 2]).await;
    crate::outbound::chat_outbound_once(&bot, &personal)
        .await
        .unwrap();
    crate::outbound::chat_outbound_once(&bot, &work)
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].to_string().contains("personal-bot"));
    assert!(requests[1].to_string().contains("work-bot"));
}

#[tokio::test]
async fn revoked_named_bot_queue_never_reaches_network() {
    for reassign in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut state = state_at(&dir.path().join("mailbox.sqlite"));
        state.interface_id = Some("private-bot".into());
        let sent = {
            let conn = state.conn.lock().await;
            team_core::interfaces::register(
                &conn,
                &team_core::interfaces::InterfaceInfo {
                    id: "private-bot".into(),
                    provider: "telegram".into(),
                    name: "Private".into(),
                    description: "Private errands".into(),
                    capabilities: vec!["text".into()],
                },
                &["p:manager".into()],
            )
            .unwrap();
            let repo = Repository::new(&conn);
            let conv = repo
                .open_interface("p:manager", "private-bot", "7", "Private")
                .unwrap();
            let sent = repo
                .reply(
                    "p:manager",
                    Some(&conv.id),
                    None,
                    "private",
                    "text",
                    None,
                    None,
                )
                .unwrap();
            conn.execute(
                "DELETE FROM agent_interfaces WHERE interface_id='private-bot'",
                [],
            )
            .unwrap();
            if reassign {
                conn.execute(
                    "INSERT INTO agent_interfaces VALUES('p:other','private-bot')",
                    [],
                )
                .unwrap();
            } else {
                conn.execute(
                    "UPDATE interface_instances SET active=0 WHERE id='private-bot'",
                    [],
                )
                .unwrap();
            }
            sent
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bot = Bot::new("123:fake").set_api_url(
            format!("http://{}/", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        );
        crate::outbound::outbound_once(&bot, &state, ChatId(7))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        let conn = state.conn.lock().await;
        assert_eq!(
            Repository::new(&conn).message(sent.id).unwrap().status,
            DeliveryStatus::Failed
        );
    }
}

#[tokio::test]
async fn native_agent_help_and_runtime_slash_commands_never_use_tmux() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("state/mailbox.sqlite"));
    std::fs::write(
        dir.path().join("team-compose.yaml"),
        "version: '3.0.0'\nprojects: [{file: p.yaml}]\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("p.yaml"),"version: 2\nproject: {id: p,name: Personal,cwd: .}\nagents:\n  manager:\n    runtime: codex\n    execution: {mode: native}\n").unwrap();
    state.compose_root = Some(dir.path().into());
    assert!(state.scoped_native().unwrap());
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![true; 2]).await;
    handle_message(bot.clone(), message(7, 1, "/help"), state.clone())
        .await
        .unwrap();
    handle_message(bot, message(7, 2, "/compact"), state.clone())
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert!(requests
        .iter()
        .all(|r| r["text"].as_str().unwrap().contains("unavailable")));
    let conn = state.conn.lock().await;
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
