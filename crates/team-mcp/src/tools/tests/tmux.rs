#[test]
fn compact_self_types_body_literally_then_submits_separately() {
    // Two-phase wire shape: `/compact` is typed with `-l` (literal),
    // then `Enter` is a SEPARATE send-keys invocation — not `\r` in the
    // same write. Codex folds a same-write `\r` into the composer
    // instead of submitting, so the split is what makes the slash
    // command actually fire.
    assert_eq!(
        compact_self_argv("t-p-mgr"),
        ["send-keys", "-t", "t-p-mgr", "-l", "/compact"]
    );
    assert_eq!(
        submit_argv("t-p-mgr"),
        ["send-keys", "-t", "t-p-mgr", "Enter"]
    );
}

#[test]
fn type_line_argv_is_literal_so_bodies_are_not_read_as_keys() {
    // `-l` guards a body containing tmux key names (Enter, C-c) or a
    // separator (;) from being interpreted — it's sent as text.
    let argv = type_line_argv("t-p-mgr", "press Enter; then C-c");
    assert_eq!(
        argv,
        ["send-keys", "-t", "t-p-mgr", "-l", "press Enter; then C-c"]
    );
}

#[test]
fn pane_session_matches_supervisor_canonical_formula() {
    // Same shape as `team-core::supervisor::AgentSpec::from_handle`
    // and `team-bot::slash_outcome` — keep the three call sites in
    // sync. If this assert ever has to change, all three move
    // together.
    assert_eq!(pane_session("t-", "p:mgr").unwrap(), "t-p-mgr");
    assert_eq!(pane_session("", "p:mgr").unwrap(), "p-mgr");
    assert_eq!(
        pane_session("teamctl-", "alpha:hugo").unwrap(),
        "teamctl-alpha-hugo"
    );
}

#[test]
fn pane_session_rejects_malformed_agent_id() {
    // The `<project>:<agent>` invariant is upheld at the MCP boundary
    // so the handler doesn't fire tmux against a garbage session
    // name. Surface the malformed id in the error so an operator
    // can trace which client misconfigured itself.
    let err = pane_session("t-", "no-colon-here").unwrap_err();
    assert!(err.contains("malformed"), "error names the gate: {err}");
    assert!(err.contains("no-colon-here"), "error names the id: {err}");
}

#[tokio::test]
async fn compact_self_dispatches_for_claude_code_manager() {
    // Happy path: a claude-code manager call returns the dispatch
    // record with the resolved session name. The actual tmux side
    // effect runs on the blocking pool and is not observed here —
    // fire-and-forget is the contract.
    let (ctx, _f) = ctx_with_manager();
    let resp = compact_self(&ctx).await.unwrap();
    assert_eq!(resp["structuredContent"]["status"], "dispatched");
    assert_eq!(resp["structuredContent"]["session"], "t-p-mgr");
}

#[tokio::test]
async fn compact_self_dispatches_for_worker_too() {
    // No manager gate: workers can self-compact when their role
    // instruction says so. The destructive warning in the schema
    // description carries the safety; the tool itself is open to
    // any claude-code agent.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
        .unwrap();
    let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
    let resp = compact_self(&ctx).await.unwrap();
    assert_eq!(resp["structuredContent"]["status"], "dispatched");
    assert_eq!(resp["structuredContent"]["session"], "t-p-dev");
}

#[tokio::test]
async fn compact_self_dispatches_for_codex_runtime() {
    // Codex's TUI ships a `/compact` slash command with the same
    // semantics as Claude Code's, so the same tmux send-keys
    // mechanism works — a codex agent passes the runtime gate and
    // gets the same dispatch record.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:mgr", "p", "P", "manager", "codex", true)
        .unwrap();
    let ctx = Ctx::new("p:mgr".to_string(), store, "t-".to_string());
    let resp = compact_self(&ctx).await.unwrap();
    assert_eq!(resp["structuredContent"]["status"], "dispatched");
    assert_eq!(resp["structuredContent"]["session"], "t-p-mgr");
}

#[tokio::test]
async fn compact_self_unsupported_runtime_is_rejected() {
    // Runtimes without a known `/compact` command (gemini here)
    // don't recognize the slash command; sending it would just land
    // as input. Reject so the failure mode is crisp and the tool's
    // description stays honest.
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:mgr", "p", "P", "manager", "gemini", true)
        .unwrap();
    let ctx = Ctx::new("p:mgr".to_string(), store, "t-".to_string());
    let err = compact_self(&ctx).await.unwrap_err();
    assert!(
        err.contains("Claude Code and Codex"),
        "error names the runtime gate: {err}"
    );
    assert!(
        err.contains("gemini"),
        "error names the actual runtime: {err}"
    );
}
