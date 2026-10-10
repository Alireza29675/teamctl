#[test]
fn thread_id_params_document_expected_carry_on() {
    // #115: thread_id carry-on is prompt-level guidance (not enforced).
    // Pin that the `dm` and `reply_to_user` thread_id params carry a
    // description framing carry-on as the norm on replies — regressing
    // `dm.thread_id` back to an empty param (the original bug) or
    // softening the wording flattens long conversations into one stream.
    let tools = schema();
    let tools = tools.as_array().expect("schema is a JSON array");
    let desc = |tool: &str, param: &str| -> String {
        tools
            .iter()
            .find(|t| t["name"] == tool)
            .and_then(|t| t["inputSchema"]["properties"][param]["description"].as_str())
            .unwrap_or_else(|| panic!("{tool}.{param} must have a description"))
            .to_string()
    };

    let dm = desc("dm", "thread_id");
    assert!(
        dm.contains("genuinely new topic") && dm.contains("thread_id"),
        "dm.thread_id must carry the load-bearing carry-on guidance, got: {dm}"
    );

    let reply = desc("reply_to_user", "thread_id");
    assert!(
        reply.contains("genuinely new topic") && reply.contains("thread_id"),
        "reply_to_user.thread_id must carry the load-bearing carry-on guidance, got: {reply}"
    );
}

#[tokio::test]
async fn reply_to_user_text_only_back_compat() {
    // Existing text-only callers see the legacy code path: kind +
    // structured_payload stay NULL, response carries `id`. Pins R5
    // back-compat from the umbrella's acceptance criteria.
    let (ctx, _f) = ctx_with_manager();
    let resp = reply_to_user(&ctx, json!({ "text": "hello" }))
        .await
        .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let (text, kind, payload) = fetch_message(&ctx.store, id);
    assert_eq!(text, "hello");
    assert!(kind.is_none(), "text-only must leave kind NULL");
    assert!(payload.is_none());
}

#[tokio::test]
async fn reply_to_user_image_url_inserts_structured_row() {
    // URL source bypasses local validation — we trust the caller and
    // let Telegram validate server-side. Pins the kind/payload columns.
    let (ctx, _f) = ctx_with_manager();
    let resp = reply_to_user(
        &ctx,
        json!({
            "image": {
                "source": "url",
                "value": "https://example.com/a.png",
                "caption": "PR ready"
            }
        }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let (text, kind, payload) = fetch_message(&ctx.store, id);
    assert_eq!(text, "PR ready", "caption mirrors into text column");
    assert_eq!(kind.as_deref(), Some("image"));
    let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
    assert_eq!(p["source"], "url");
    assert_eq!(p["value"], "https://example.com/a.png");
    assert_eq!(p["caption"], "PR ready");
}

#[tokio::test]
async fn reply_to_user_image_path_round_trip() {
    // path source: real file on disk under the size cap and within
    // the mime allowlist. Pins the kind=image row + the structured
    // payload string the bot's outbound dispatcher will parse.
    let (ctx, _f) = ctx_with_manager();
    let img = NamedTempFile::with_suffix(".png").unwrap();
    std::fs::write(img.path(), b"not really a png").unwrap();
    let resp = reply_to_user(
        &ctx,
        json!({
            "image": {
                "source": "path",
                "value": img.path().to_str().unwrap(),
                "caption": "screenshot"
            }
        }),
    )
    .await
    .unwrap();
    let id = resp["structuredContent"]["id"].as_i64().unwrap();
    let (_, kind, payload) = fetch_message(&ctx.store, id);
    assert_eq!(kind.as_deref(), Some("image"));
    let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
    assert_eq!(p["source"], "path");
    assert_eq!(p["value"], img.path().to_str().unwrap());
}

#[tokio::test]
async fn reply_to_user_text_plus_image_inserts_two_rows() {
    // Multi-content shape: one tool call → one text row + one image
    // row, returned as the `ids` array. Order is text-first, image-
    // next so the operator reads the framing line before the photo.
    let (ctx, _f) = ctx_with_manager();
    let resp = reply_to_user(
        &ctx,
        json!({
            "text": "here's the latest design",
            "image": { "source": "url", "value": "https://example.com/d.png" }
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
    let (text0, kind0, _) = fetch_message(&ctx.store, ids[0]);
    let (_, kind1, _) = fetch_message(&ctx.store, ids[1]);
    assert_eq!(text0, "here's the latest design");
    assert!(kind0.is_none(), "first row is text");
    assert_eq!(kind1.as_deref(), Some("image"));
}

#[tokio::test]
async fn reply_to_user_rejects_disallowed_image_extension() {
    let (ctx, _f) = ctx_with_manager();
    let f = NamedTempFile::with_suffix(".bmp").unwrap();
    std::fs::write(f.path(), b"x").unwrap();
    let err = reply_to_user(
        &ctx,
        json!({ "image": { "source": "path", "value": f.path().to_str().unwrap() } }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("allowlist"),
        "error must name the mime allowlist: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_rejects_oversize_path() {
    let (ctx, _f) = ctx_with_manager();
    let big = NamedTempFile::with_suffix(".png").unwrap();
    // Sparse-write a file 1 byte past the 50MB cap — fast and doesn't
    // need the bytes to be real.
    big.as_file()
        .set_len(MEDIA_MAX_BYTES + 1)
        .expect("sparse extend");
    let err = reply_to_user(
        &ctx,
        json!({ "image": { "source": "path", "value": big.path().to_str().unwrap() } }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("50MB"),
        "error must reference the size cap: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_rejects_missing_path() {
    let (ctx, _f) = ctx_with_manager();
    let err = reply_to_user(
        &ctx,
        json!({
            "image": { "source": "path", "value": "/nonexistent/thing.png" }
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("not readable"),
        "error must name the unreadable path: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_rejects_empty_call() {
    let (ctx, _f) = ctx_with_manager();
    let err = reply_to_user(&ctx, json!({})).await.unwrap_err();
    assert!(
        err.contains("at least one"),
        "empty call must surface the at-least-one constraint: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_accepts_text_at_exactly_the_limit() {
    // Boundary: 4096 is allowed (Telegram rejects > 4096, not >=).
    let (ctx, _f) = ctx_with_manager();
    let at_limit = "a".repeat(MAX_TELEGRAM_TEXT_LEN);
    let resp = reply_to_user(&ctx, json!({ "text": at_limit }))
        .await
        .expect("text at exactly the limit must pass");
    assert!(resp["structuredContent"]["id"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn reply_to_user_rejects_text_over_limit_with_actionable_error() {
    let (ctx, _f) = ctx_with_manager();
    let over = "a".repeat(MAX_TELEGRAM_TEXT_LEN + 1);
    let err = reply_to_user(&ctx, json!({ "text": over }))
        .await
        .unwrap_err();
    assert!(err.contains("4096"), "error must name the limit: {err}");
    assert!(
        err.contains("4097"),
        "error must report the actual size: {err}"
    );
    assert!(
        err.contains("Split into multiple"),
        "error must name the chunk remediation: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_text_length_is_utf16_not_bytes() {
    // A string whose UTF-8 byte length blows past 4096 but whose
    // UTF-16 length (what Telegram counts) is well under it must NOT
    // be rejected. '✅' is 3 bytes UTF-8, 1 UTF-16 unit: 2000 of them
    // = 6000 bytes but 2000 UTF-16 units. Pins the load-bearing
    // measure choice — a naive `.len()` check would false-reject this.
    let (ctx, _f) = ctx_with_manager();
    let emoji_heavy = "✅".repeat(2000);
    assert!(
        emoji_heavy.len() > MAX_TELEGRAM_TEXT_LEN,
        "precondition: byte len exceeds limit"
    );
    let resp = reply_to_user(&ctx, json!({ "text": emoji_heavy }))
        .await
        .expect("UTF-16-counted length is under limit; must pass");
    assert!(resp["structuredContent"]["id"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn reply_to_user_rejects_oversize_image_caption() {
    let (ctx, _f) = ctx_with_manager();
    let big_caption = "c".repeat(MAX_TELEGRAM_CAPTION_LEN + 1);
    let err = reply_to_user(
        &ctx,
        json!({
            "image": {
                "source": "url",
                "value": "https://example.com/a.png",
                "caption": big_caption
            }
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("image.caption") && err.contains("1024"),
        "error must name the field and the caption limit: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_rejects_oversize_file_caption() {
    let (ctx, _f) = ctx_with_manager();
    let big_caption = "c".repeat(MAX_TELEGRAM_CAPTION_LEN + 1);
    let err = reply_to_user(
        &ctx,
        json!({
            "file": {
                "source": "url",
                "value": "https://example.com/a.pdf",
                "caption": big_caption
            }
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("file.caption") && err.contains("1024"),
        "error must name the field and the caption limit: {err}"
    );
}

#[tokio::test]
async fn reply_to_user_non_manager_is_rejected_before_validation() {
    // R8 manager-gating: a worker call hits the is_manager check
    // first, so we never even read the disk for path-source media.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
    let err = reply_to_user(
        &ctx,
        json!({
            "image": { "source": "path", "value": "/nonexistent/x.png" }
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("only managers"),
        "non-manager must be gated before media validation: {err}"
    );
}

#[test]
fn image_extension_allowlist_accepts_canonical_set_and_rejects_others() {
    for ok in [
        "/tmp/a.jpg",
        "/tmp/a.JPEG",
        "/tmp/photo.PNG",
        "/tmp/sticker.webp",
        "/tmp/loop.gif",
    ] {
        assert!(image_extension_allowed(ok), "should accept: {ok}");
    }
    for bad in ["/tmp/a.bmp", "/tmp/a.tiff", "/tmp/a.svg", "/tmp/no_ext"] {
        assert!(!image_extension_allowed(bad), "should reject: {bad}");
    }
}

// ── T-086-E react_to_user ──────────────────────────────────────
