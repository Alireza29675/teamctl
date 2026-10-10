#[test]
fn agent_runtime_returns_runtime_for_known_agent() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // `seed` inserts p:eng_lead (manager, runtime "claude-code"),
    // p:pm (manager), and p:dev1 (worker).
    assert_eq!(
        agent_runtime(&conn, "p:eng_lead"),
        Some("claude-code".into())
    );
}

#[test]
fn agent_runtime_returns_runtime_when_runtime_varies() {
    // Hand-extend the seed with a non-CC manager so the lookup
    // path is exercised against a runtime that the slash-passthrough
    // gate would later reject.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:codex_mgr','p','codex_mgr','codex',1,NULL)",
        [],
    )
    .unwrap();
    assert_eq!(agent_runtime(&conn, "p:codex_mgr"), Some("codex".into()));
}

#[test]
fn agent_runtime_returns_none_for_unknown_agent() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    assert_eq!(agent_runtime(&conn, "p:ghost"), None);
}

// ── T-367: first-connect greeting + /help split ──────────────────

#[test]
fn start_greeting_is_one_liner_using_manager_id_fallback() {
    // /start on a scoped bot without a display_name: short one-liner
    // naming the manager id, and crucially NO command dump.
    let body = start_help_body(false, Some("software-team:director"));
    assert_eq!(
        body,
        "Connected to software-team:director via teamctl. Just type to chat."
    );
    assert!(
        !body.contains("/pending"),
        "greeting must not dump commands"
    );
    assert!(!body.contains("/dm"), "greeting must not dump commands");
}

#[test]
fn start_greeting_prefers_display_name() {
    // /start prefers the friendly display_name (T-160) when set.
    let body = start_help_body(false, Some("Director"));
    assert_eq!(
        body,
        "Connected to Director via teamctl. Just type to chat."
    );
}

#[test]
fn start_greeting_unscoped_points_to_help() {
    // Unscoped bot has no single manager to name — still a one-liner,
    // and it points the operator at /help for the command list.
    let body = start_help_body(false, None);
    assert_eq!(body, "Connected via teamctl. Send /help for commands.");
    assert!(!body.contains("/dm"), "greeting must not dump commands");
}

#[test]
fn help_lists_advanced_commands_for_scoped_bot() {
    // /help is where the power-user commands live now.
    let body = start_help_body(true, Some("Director"));
    assert!(body.contains("/pending"), "help must list /pending");
    assert!(body.contains("/dm"), "help must list /dm");
    assert!(
        body.contains("slash-passthrough to Director's tmux session (Claude Code and Codex)"),
        "help must list slash-passthrough naming the manager: {body}"
    );
    assert!(
        body.contains("chat with Director"),
        "help should still tell the operator how to chat: {body}"
    );
}

#[test]
fn help_lists_dm_and_pending_for_unscoped_bot() {
    // Unscoped /help keeps the dm + pending commands discoverable;
    // slash-passthrough is manager-scoped only, so it's absent here.
    let body = start_help_body(true, None);
    assert!(
        body.contains("/dm <project>:<agent>"),
        "unscoped help must list /dm"
    );
    assert!(
        body.contains("/pending"),
        "unscoped help must list /pending"
    );
    assert!(
        !body.contains("slash-passthrough"),
        "unscoped bot has no manager tmux session to pass through to: {body}"
    );
}
