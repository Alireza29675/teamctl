fn fetch_kind_and_payload(store: &Store, id: i64) -> (Option<String>, Option<String>) {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT kind, structured_payload FROM messages WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

#[tokio::test]
async fn react_to_user_persists_kind_and_structured_payload() {
    // Affirmative path: agent calls with a supported emoji + valid
    // message id; row lands with `kind = "reaction"` and a payload
    // carrying both fields the bot dispatcher needs.
    let (ctx, _f) = ctx_with_manager();
    let resp = react_to_user(&ctx, json!({ "telegram_msg_id": 4242, "emoji": "👀" }))
        .await
        .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let (kind, payload) = fetch_kind_and_payload(&ctx.store, id);
    assert_eq!(kind.as_deref(), Some("reaction"));
    let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
    assert_eq!(p["telegram_msg_id"], 4242);
    assert_eq!(p["emoji"], "👀");
}

#[tokio::test]
async fn react_to_user_returns_message_id_and_emoji_in_response() {
    // The structured response surfaces both the new mailbox row id
    // and the (telegram_msg_id, emoji) pair so callers can correlate
    // their request with the eventual outbound API call.
    let (ctx, _f) = ctx_with_manager();
    let resp = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🎉" }))
        .await
        .unwrap();
    assert_eq!(resp["structuredContent"]["telegram_msg_id"], 7);
    assert_eq!(resp["structuredContent"]["emoji"], "🎉");
    assert_eq!(resp["structuredContent"]["recipient"], "user:telegram");
}

#[tokio::test]
async fn react_to_user_rejects_out_of_allowlist_emoji() {
    // Defence in depth: out-of-set emoji surfaces a clean MCP error
    // rather than reaching Telegram and getting a server-side
    // rejection that would land in the bot's tracing::warn! log
    // instead of the agent's tool-call response.
    let (ctx, _f) = ctx_with_manager();
    let err = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🍕" }))
        .await
        .unwrap_err();
    assert!(err.contains("allowlist"), "error names the gate: {err}");
    assert!(
        err.contains("🍕"),
        "error includes the rejected emoji: {err}"
    );
}

#[tokio::test]
async fn react_to_user_non_manager_is_rejected_before_validation() {
    // R8 manager-gating: a worker call hits the is_manager check
    // first, so we never reach allowlist validation. Mirrors the
    // PR #81 (T-086-A) reply_to_user gating shape.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
    let err = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🍕" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("only managers"),
        "non-manager must be gated before allowlist check: {err}"
    );
}

#[tokio::test]
async fn react_to_user_rejects_missing_telegram_msg_id() {
    // Schema's `required: ["telegram_msg_id", "emoji"]` is
    // enforced at deserialization time; pinning the rejection so a
    // future schema-shape regression surfaces here.
    let (ctx, _f) = ctx_with_manager();
    let err = react_to_user(&ctx, json!({ "emoji": "👍" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("telegram_msg_id") || err.contains("missing"),
        "missing required field error: {err}"
    );
}

// ── T-102 show_typing ──────────────────────────────────────────

#[tokio::test]
async fn show_typing_persists_kind_and_empty_payload() {
    // Affirmative path: a manager-side call lands a row keyed by
    // `kind = "typing"` with an empty JSON payload. The bot's
    // dispatcher discriminates on `kind` alone, so the payload only
    // has to be valid JSON.
    let (ctx, _f) = ctx_with_manager();
    let resp = show_typing(&ctx, json!({})).await.unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let (kind, payload) = fetch_kind_and_payload(&ctx.store, id);
    assert_eq!(kind.as_deref(), Some("typing"));
    assert_eq!(payload.as_deref(), Some("{}"));
}

#[tokio::test]
async fn show_typing_returns_message_id_and_recipient() {
    // Response shape: callers get the new mailbox row id back so
    // they can correlate it (mostly useful in tests + diagnostics —
    // production agents fire-and-forget) and the recipient string
    // matches the same constant `reply_to_user` and `react_to_user`
    // emit, so a single response shape spans the manager-side
    // tools.
    let (ctx, _f) = ctx_with_manager();
    let resp = show_typing(&ctx, json!({})).await.unwrap();
    assert!(resp["structuredContent"]["id"].as_i64().is_some());
    assert_eq!(resp["structuredContent"]["recipient"], "user:telegram");
}

#[tokio::test]
async fn show_typing_non_manager_is_rejected() {
    // Manager gate: workers can't open a typing window. Mirrors the
    // `reply_to_user` / `react_to_user` gating so the manager-only
    // surface stays consistent.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
    let err = show_typing(&ctx, json!({})).await.unwrap_err();
    assert!(
        err.contains("only managers"),
        "non-manager must be gated: {err}"
    );
}

// ── T-109 compact_self ─────────────────────────────────────────
