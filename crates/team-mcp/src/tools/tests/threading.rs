#[test]
fn bot_reaction_allowlist_accepts_canonical_set_and_rejects_pizza() {
    // Spot-check both directions on the in-memory allowlist —
    // canonical entries pass; an obvious non-entry fails.
    for ok in ["👍", "👎", "❤️", "🎉", "👀", "🤝", "👨\u{200d}💻"] {
        assert!(is_allowed_reaction(ok), "should accept: {ok}");
    }
    for bad in ["🍕", "🥑", "abc", ""] {
        assert!(!is_allowed_reaction(bad), "should reject: {bad}");
    }
}

// ── T-168 reply_to_message_id (mailbox-id semantics) ───────────

fn fetch_telegram_msg_id(store: &Store, id: i64) -> Option<i64> {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT telegram_msg_id FROM messages WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

/// Hand-insert an inbound `user:telegram` row with a known
/// `telegram_msg_id` and return its mailbox id — what an agent
/// would see as `meta.id` in the channel envelope.
fn seed_inbound_row(store: &Store, telegram_msg_id: i64) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, ?1)",
        params![telegram_msg_id],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[tokio::test]
async fn reply_to_user_threads_text_when_reply_to_message_id_set() {
    // T-168 happy path: agent passes the inbound row's mailbox id
    // (`meta.id`); the store resolves it to the row's Telegram id
    // at insert and persists THAT on the outbound row so the bot's
    // dispatcher can hand it to `reply_parameters`.
    let (ctx, _f) = ctx_with_manager();
    let inbound = seed_inbound_row(&ctx.store, 12345);
    let resp = reply_to_user(
        &ctx,
        json!({ "text": "ack", "reply_to_message_id": inbound }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    assert_eq!(fetch_telegram_msg_id(&ctx.store, id), Some(12345));
}

#[tokio::test]
async fn reply_to_user_text_only_back_compat_leaves_telegram_msg_id_null() {
    // R5 back-compat: existing callers that don't pass
    // `reply_to_message_id` see the same NULL column they always
    // did. Bot dispatcher reads NULL → omits `reply_parameters` →
    // message lands as a fresh post.
    let (ctx, _f) = ctx_with_manager();
    let resp = reply_to_user(&ctx, json!({ "text": "ack" })).await.unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    assert!(fetch_telegram_msg_id(&ctx.store, id).is_none());
}

#[tokio::test]
async fn reply_to_user_threads_image_when_reply_to_message_id_set() {
    // Threading + media: image attached as a reply nests under
    // the parent message in Telegram. The outbound media row
    // carries the resolved `telegram_msg_id` the text path does.
    let (ctx, _f) = ctx_with_manager();
    let inbound = seed_inbound_row(&ctx.store, 7);
    let resp = reply_to_user(
        &ctx,
        json!({
            "image": {
                "source": "url",
                "value": "https://example.com/a.png",
                "caption": "screenshot"
            },
            "reply_to_message_id": inbound
        }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    assert_eq!(fetch_telegram_msg_id(&ctx.store, id), Some(7));
}

#[tokio::test]
async fn reply_to_user_text_plus_image_share_one_reply_to_message_id() {
    // Multi-content shape: one tool call → two outbound rows
    // (text + image) — both resolve from the same inbound mailbox
    // id, so the operator sees both replies threaded under the
    // same parent in Telegram, not split into two separate threads.
    let (ctx, _f) = ctx_with_manager();
    let inbound = seed_inbound_row(&ctx.store, 99);
    let resp = reply_to_user(
        &ctx,
        json!({
            "text": "fixing",
            "image": { "source": "url", "value": "https://example.com/d.png" },
            "reply_to_message_id": inbound
        }),
    )
    .await
    .unwrap();
    let ids: Vec<i64> = resp["structuredContent"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert_eq!(
            fetch_telegram_msg_id(&ctx.store, id),
            Some(99),
            "every row in the call shares the same reply target"
        );
    }
}

#[tokio::test]
async fn reply_to_user_resolution_misses_leave_telegram_msg_id_null() {
    // T-168 miss path: agent references a mailbox id that doesn't
    // resolve (stale id, or row without telegram_msg_id). The
    // outbound row sends without threading rather than failing
    // the call — observability comes from the warn-log in
    // resolve_telegram_msg_id, not a tool-level error.
    let (ctx, _f) = ctx_with_manager();
    let resp = reply_to_user(
        &ctx,
        json!({ "text": "ack", "reply_to_message_id": 999_999 }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    assert!(fetch_telegram_msg_id(&ctx.store, id).is_none());
}

// ── T-32b read_attachment ──────────────────────────────────────
