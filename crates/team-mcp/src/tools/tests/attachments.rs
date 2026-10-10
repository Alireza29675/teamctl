fn ctx_with_attachments(
    cfg: team_core::compose::Attachments,
) -> (Ctx, NamedTempFile, tempfile::TempDir) {
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let compose_root = tempfile::tempdir().unwrap();
    let ctx =
        Ctx::for_test_with_attachments("p:dev".to_string(), store, compose_root.path(), cfg, None);
    (ctx, f, compose_root)
}

fn cfg_with_root_only(root: &std::path::Path) -> team_core::compose::Attachments {
    team_core::compose::Attachments {
        enabled: true,
        max_size_bytes: 1024 * 1024,
        allowed_roots: vec![root.to_string_lossy().into_owned()],
        scanner: None,
        audit_log_path: None,
        tempfile_ttl_seconds: 6 * 60 * 60,
    }
}

#[tokio::test]
async fn read_attachment_no_compose_returns_disabled() {
    // Hand-launched / no-compose-root case: tool returns the
    // disabled-style envelope rather than touching the
    // filesystem. Pinning so a refactor that removes the early
    // exit doesn't silently start serving raw bytes.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let ctx = Ctx::new("p:dev".to_string(), store, "t-".into());
    let resp = read_attachment(&ctx, json!({ "path": "/etc/passwd" }))
        .await
        .unwrap();
    assert_eq!(resp["structuredContent"]["rejected"], true);
    let reason = resp["structuredContent"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("attachments unavailable"),
        "reason: {reason}"
    );
}

#[tokio::test]
async fn read_attachment_happy_path_stages_and_returns_path() {
    let work = tempfile::tempdir().unwrap();
    let payload = work.path().join("note.md");
    std::fs::write(&payload, b"the build is green").unwrap();
    let cfg = cfg_with_root_only(work.path());
    let (ctx, _f, compose_root) = ctx_with_attachments(cfg);
    let resp = read_attachment(&ctx, json!({ "path": payload.display().to_string() }))
        .await
        .unwrap();
    assert_eq!(resp["structuredContent"]["rejected"], false);
    let temp_path = resp["structuredContent"]["temp_path"].as_str().unwrap();
    let staged_bytes = std::fs::read(temp_path).unwrap();
    assert_eq!(staged_bytes, b"the build is green");
    // Staged under the compose root's staging dir.
    assert!(
        temp_path.starts_with(
            compose_root
                .path()
                .join("state/attachments-staging")
                .to_str()
                .unwrap()
        ),
        "staged path under compose root: {temp_path}"
    );
    // Response carries the blake3 + size for the agent's audit
    // hooks (and to detect mid-stream tampering).
    assert!(resp["structuredContent"]["blake3"].is_string());
    assert_eq!(resp["structuredContent"]["size"].as_u64(), Some(18));
}

#[tokio::test]
async fn read_attachment_reject_writes_telegram_and_wire_rows() {
    // Outside-allowed-roots reject: agent gets the rejection
    // envelope, AND two notification rows land in the mailbox —
    // one to user:telegram for the team-bot to forward, one to
    // channel:p:all so the operator's TUI Wire tab surfaces the
    // reason. Owner-ratify variant 3 (option c) requires both.
    let inside = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let leak = outside.path().join("leak.txt");
    std::fs::write(&leak, b"x").unwrap();
    let cfg = cfg_with_root_only(inside.path());
    let (ctx, _f, _compose_root) = ctx_with_attachments(cfg);
    let resp = read_attachment(&ctx, json!({ "path": leak.display().to_string() }))
        .await
        .unwrap();
    assert_eq!(resp["structuredContent"]["rejected"], true);

    // Inspect mailbox rows: expect exactly two reject rows from
    // the broker (telegram + wire), text containing the path.
    let conn = ctx.store.conn.lock().unwrap();
    let telegram_sender: String = conn
        .query_row(
            "SELECT sender FROM messages WHERE recipient='user:telegram'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        telegram_sender, ctx.agent_id,
        "bot routing follows the requesting agent's owner"
    );
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT recipient, text FROM messages ORDER BY id ASC")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .flatten()
        .collect();
    let recipients: Vec<&str> = rows.iter().map(|(r, _)| r.as_str()).collect();
    assert!(
        recipients.contains(&"user:telegram"),
        "telegram reject row: {recipients:?}"
    );
    assert!(
        recipients.contains(&"channel:p:all"),
        "wire reject row: {recipients:?}"
    );
    // Both notifications mention the operator-supplied path.
    for (_, text) in &rows {
        assert!(
            text.contains(&leak.display().to_string()),
            "reject text mentions path: {text}"
        );
    }
}

#[tokio::test]
async fn read_attachment_disabled_returns_reject_without_filesystem_touch() {
    let work = tempfile::tempdir().unwrap();
    let payload = work.path().join("note.md");
    std::fs::write(&payload, b"x").unwrap();
    let mut cfg = cfg_with_root_only(work.path());
    cfg.enabled = false;
    let (ctx, _f, _root) = ctx_with_attachments(cfg);
    let resp = read_attachment(&ctx, json!({ "path": payload.display().to_string() }))
        .await
        .unwrap();
    assert_eq!(resp["structuredContent"]["rejected"], true);
    let reason = resp["structuredContent"]["reason"].as_str().unwrap();
    assert!(reason.contains("disabled"), "reason: {reason}");
}

#[tokio::test]
async fn read_attachment_audit_log_records_accept_and_reject() {
    // Two attempts, one accept + one reject, both lines land
    // in the audit file. Each line is parseable JSON.
    let work = tempfile::tempdir().unwrap();
    let ok_path = work.path().join("ok.md");
    std::fs::write(&ok_path, b"hi").unwrap();
    let bogus = work.path().join("missing.md");
    let mut cfg = cfg_with_root_only(work.path());
    let audit_path = work.path().join("audit/attempts.log");
    cfg.audit_log_path = Some(audit_path.clone());
    let (ctx, _f, _root) = ctx_with_attachments(cfg);
    let _ = read_attachment(&ctx, json!({ "path": ok_path.display().to_string() }))
        .await
        .unwrap();
    let _ = read_attachment(&ctx, json!({ "path": bogus.display().to_string() }))
        .await
        .unwrap();
    let body = std::fs::read_to_string(&audit_path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2, "two attempts, two lines: {body}");
    let outcomes: Vec<String> = lines
        .iter()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["outcome"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(outcomes, vec!["accept", "reject"]);
}

#[tokio::test]
async fn read_attachment_missing_path_arg_is_rejected_at_schema() {
    // `required: ["path"]` enforced at deserialization; a
    // schema-shape regression would surface as a deserialize
    // error here, not as an MCP-level success.
    let (ctx, _f, _root) =
        ctx_with_attachments(cfg_with_root_only(tempfile::tempdir().unwrap().path()));
    let err = read_attachment(&ctx, json!({})).await.unwrap_err();
    assert!(
        err.contains("path") || err.contains("missing"),
        "missing required field error: {err}"
    );
}

// ── #299 multi-option request_approval ───────────────────────
