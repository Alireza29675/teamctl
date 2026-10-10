fn approval_options_json(store: &Store, id: i64) -> Option<String> {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT options_json FROM approvals WHERE id=?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn request_approval_rejects_unrepresentable_ttl_before_inserting() {
    let (ctx, _file) = ctx_with_manager();
    let result = request_approval(
        &ctx,
        json!({
            "action":"deploy", "summary":"invalid TTL", "ttl_seconds":u64::MAX,
        }),
    )
    .await;
    assert!(result.unwrap_err().contains("ttl_seconds is too large"));
    let count: i64 = ctx
        .store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM approvals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn request_approval_binary_is_unchanged() {
    // No `options` → options_json NULL (bot renders Approve/Deny)
    // and the response carries a null `value`. Pins back-compat.
    let (ctx, _f) = ctx_with_manager();
    let resp = request_approval(
        &ctx,
        json!({ "action": "deploy", "summary": "ship it?", "wait": false }),
    )
    .await
    .unwrap();
    let sc = &resp["structuredContent"];
    assert_eq!(sc["status"], "pending");
    assert!(sc["value"].is_null(), "binary response value must be null");
    let id = sc["id"].as_i64().unwrap();
    assert!(
        approval_options_json(&ctx.store, id).is_none(),
        "binary request must leave options_json NULL"
    );
}

#[tokio::test]
async fn request_approval_with_options_serializes_them() {
    let (ctx, _f) = ctx_with_manager();
    let resp = request_approval(
        &ctx,
        json!({
            "action": "decide",
            "summary": "which rollout?",
            "wait": false,
            "options": [
                { "label": "Canary", "value": "canary" },
                { "label": "Full",   "value": "full" },
                { "label": "Hold",   "value": "hold" }
            ]
        }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let stored = approval_options_json(&ctx.store, id).expect("options_json set");
    let parsed: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 3);
    assert_eq!(parsed[0]["label"], "Canary");
    assert_eq!(parsed[0]["value"], "canary");
    assert_eq!(parsed[2]["value"], "hold");
}

#[tokio::test]
async fn request_approval_empty_options_falls_back_to_binary() {
    // An explicit empty array is treated as "no options" — the bot
    // would otherwise render a Cancel-only card with no real choice.
    let (ctx, _f) = ctx_with_manager();
    let resp = request_approval(
        &ctx,
        json!({ "action": "deploy", "summary": "?", "wait": false, "options": [] }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    assert!(
        approval_options_json(&ctx.store, id).is_none(),
        "empty options array must leave options_json NULL"
    );
}
