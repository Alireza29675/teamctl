use super::*;

fn fixture() -> (tempfile::TempDir, Compose, Connection) {
    let (dir, mut compose) = super::super::up::regression_tests::fixture();
    compose.projects[0].managers.get_mut("a").unwrap().execution = Some(Execution {
        mode: ExecutionMode::Native,
        ..Default::default()
    });
    let conn = team_core::chat::open(compose.root.join("state/mailbox.db")).unwrap();
    conn.execute("INSERT INTO projects(id,name) VALUES('p','p')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO agents(id,project_id,role,runtime) VALUES('p:a','p','agent','codex')",
        [],
    )
    .unwrap();
    team_core::interfaces::sync(&conn, &compose).unwrap();
    (dir, compose, conn)
}

fn inbound(conn: &Connection, external: &str) -> Incoming {
    team_core::chat::Repository::new(conn)
        .ingest(team_core::chat::InboundMessage {
            adapter_id: "web".into(),
            peer_id: "operator".into(),
            sender_id: "operator".into(),
            agent_id: "p:a".into(),
            external_id: external.into(),
            text: "hello".into(),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap();
    next_message(conn, "p:a").unwrap().unwrap()
}

#[test]
fn automatic_final_reaches_original_web_conversation_and_acks_only_after_success() {
    let (_dir, compose, conn) = fixture();
    let message = inbound(&conn, "1");
    assert!(message
        .interface
        .as_ref()
        .unwrap()
        .contains("Web workspace"));
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    finish_message(&conn, h, &message, "hi", 0).unwrap();
    assert!(next_message(&conn, "p:a").unwrap().is_none());
    let history = team_core::chat::Repository::new(&conn)
        .history(message.conversation.as_ref().unwrap(), None, None, 50)
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].text, "hi");
    assert_eq!(history[1].status, team_core::chat::DeliveryStatus::Sent);
}

#[test]
fn explicit_rich_reply_suppresses_automatic_plain_final() {
    let (_dir, compose, conn) = fixture();
    let message = inbound(&conn, "1");
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    team_core::chat::Repository::new(&conn)
        .reply(
            &h.id(),
            message.conversation.as_deref(),
            Some(message.id),
            "**rich**",
            "markdown",
            None,
            Some("explicit"),
        )
        .unwrap();
    finish_message(&conn, h, &message, "already sent", 0).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE direction='agent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn typing_controls_do_not_suppress_the_automatic_final() {
    let (_dir, compose, conn) = fixture();
    let message = inbound(&conn, "1");
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    let repo = team_core::chat::Repository::new(&conn);
    repo.reply(
        &h.id(),
        message.conversation.as_deref(),
        Some(message.id),
        "",
        "typing",
        Some(serde_json::json!({"expires_at":now()+10.0})),
        Some("typing"),
    )
    .unwrap();
    finish_message(&conn, h, &message, "actual response", 0).unwrap();
    let history = repo
        .history(message.conversation.as_ref().unwrap(), None, None, 50)
        .unwrap();
    assert_eq!(history.last().unwrap().text, "actual response");
}

#[test]
fn another_agents_response_cannot_suppress_this_agents_final() {
    let (_dir, compose, conn) = fixture();
    let message = inbound(&conn, "a-1");
    conn.execute(
        "INSERT INTO agents(id,project_id,role,runtime) VALUES('p:b','p','agent','codex')",
        [],
    )
    .unwrap();
    let repo = team_core::chat::Repository::new(&conn);
    let b = repo
        .ingest(team_core::chat::InboundMessage {
            adapter_id: "web".into(),
            peer_id: "operator".into(),
            sender_id: "operator".into(),
            agent_id: "p:b".into(),
            external_id: "b-1".into(),
            text: "hello".into(),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap();
    repo.reply(
        "p:b",
        Some(&b.conversation_id),
        b.mailbox_id,
        "from b",
        "text",
        None,
        Some("b-final"),
    )
    .unwrap();
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    finish_message(&conn, h, &message, "from a", 0).unwrap();
    let history = repo
        .history(message.conversation.as_ref().unwrap(), None, None, 20)
        .unwrap();
    assert_eq!(history[1].text, "from a");
}

#[test]
fn agent_ignores_its_own_broadcast() {
    let (_dir, _compose, conn) = fixture();
    conn.execute(
        "INSERT INTO channels(id,project_id,name) VALUES('p:all','p','all')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channel_members(channel_id,agent_id) VALUES('p:all','p:a')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO messages(project_id,sender,recipient,text,sent_at) VALUES('p','p:a','channel:p:all','own broadcast',1)",[]).unwrap();
    assert!(next_message(&conn, "p:a").unwrap().is_none());
}

#[test]
fn native_channel_receipt_does_not_consume_another_subscriber() {
    let (_dir, compose, conn) = fixture();
    conn.execute(
        "INSERT INTO channels(id,project_id,name) VALUES('p:all','p','all')",
        [],
    )
    .unwrap();
    for agent in ["p:a", "p:b"] {
        conn.execute(
            "INSERT INTO channel_members(channel_id,agent_id) VALUES('p:all',?1)",
            [agent],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO messages(project_id,sender,recipient,text,sent_at) VALUES('p','p:c','channel:p:all','hello',1)",[]).unwrap();
    let message = next_message(&conn, "p:a").unwrap().unwrap();
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    finish_message(&conn, h, &message, "done", 0).unwrap();
    assert!(next_message(&conn, "p:a").unwrap().is_none());
    assert!(next_message(&conn, "p:b").unwrap().is_some());
}

#[test]
fn parsers_extract_sessions_and_reject_provider_errors() {
    let c = turn::parse(
        "claude-code",
        r#"{"result":"hi","session_id":"claude-id","is_error":false}"#,
    )
    .unwrap();
    assert_eq!(c.session.as_deref(), Some("claude-id"));
    let c=turn::parse("codex","{\"type\":\"thread.started\",\"thread_id\":\"codex-id\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hi\"}}\n{\"type\":\"turn.completed\"}").unwrap();
    assert_eq!(c.text, "hi");
    assert_eq!(c.session.as_deref(), Some("codex-id"));
    let c = turn::parse(
        "opencode",
        r#"{"type":"text","sessionID":"oc-id","part":{"text":"hi"}}"#,
    )
    .unwrap();
    assert_eq!(c.session.as_deref(), Some("oc-id"));
    assert!(turn::parse(
        "claude-code",
        r#"{"is_error":true,"result":"unauthorized"}"#
    )
    .is_err());
    assert!(turn::parse(
        "codex",
        r#"{"type":"turn.failed","error":{"message":"quota"}}"#
    )
    .is_err());
    assert!(turn::parse("opencode", r#"{"type":"error","error":"quota"}"#).is_err());
    assert!(turn::parse("codex", "not json").is_err());
}

#[test]
fn provider_resume_uses_explicit_agent_session_without_latest_global_history() {
    let env = BTreeMap::new();
    for runtime in ["claude-code", "codex", "opencode"] {
        let args = turn::provider_args(runtime, &env, Some("agent-session")).unwrap();
        assert!(args.iter().any(|arg| arg == "agent-session"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--last" || arg == "--continue"));
    }
}

#[test]
fn codex_explicit_bypass_does_not_conflict_with_approval_flags() {
    let env = BTreeMap::from([("PERMISSION_MODE".into(), "bypassPermissions".into())]);
    let args = turn::provider_args("codex", &env, None).unwrap();
    assert!(args
        .iter()
        .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"));
    assert!(!args
        .iter()
        .any(|arg| arg == "-a" || arg.contains("sandbox_mode")));
}

#[cfg(unix)]
#[test]
fn fake_provider_round_trip_passes_input_and_persists_exact_resume_argument() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, compose, _conn) = fixture();
    let binary = compose.root.join("fake-codex");
    std::fs::write(&binary,"#!/bin/sh\ncat > \"$0.input\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"fixture-session\"}' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"fixture reply\"}}' '{\"type\":\"turn.completed\"}'\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(compose.root.join("runtimes")).unwrap();
    std::fs::write(
        compose.root.join("runtimes/codex.yaml"),
        format!("binary: {}\nsupports_mcp: true\n", binary.display()),
    )
    .unwrap();
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    let (env, _) = team_core::render::render_agent(&compose, h, "team-mcp");
    team_core::persistence::atomic_write(
        &team_core::render::env_path(&compose.root, h.project, h.agent),
        env.as_bytes(),
    )
    .unwrap();
    let result = turn::run_turn(
        &compose,
        h,
        h.spec.execution.as_ref().unwrap(),
        Some("saved-session"),
        "fixture input",
    )
    .unwrap();
    assert_eq!(result.text, "fixture reply");
    assert_eq!(result.session.as_deref(), Some("fixture-session"));
    assert_eq!(
        std::fs::read_to_string(binary.with_extension("input")).unwrap(),
        "fixture input"
    );
    assert!(std::fs::read_to_string(binary.with_extension("args"))
        .unwrap()
        .contains("resume\nsaved-session\n"));
}

#[cfg(unix)]
#[test]
fn failed_provider_stdout_preserves_auth_reason_without_logging_headers() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, mut compose, _conn) = fixture();
    compose.projects[0].managers.get_mut("a").unwrap().runtime = "opencode".into();
    let binary = compose.root.join("fake-opencode");
    std::fs::write(&binary,"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"error\",\"error\":{\"data\":{\"message\":\"credential expired\",\"responseHeaders\":{\"Authorization\":\"fixture-do-not-log\"}}}}'\nexit 1\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(compose.root.join("runtimes")).unwrap();
    std::fs::write(
        compose.root.join("runtimes/opencode.yaml"),
        format!("binary: {}\nsupports_mcp: true\n", binary.display()),
    )
    .unwrap();
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    let (env, _) = team_core::render::render_agent(&compose, h, "team-mcp");
    team_core::persistence::atomic_write(
        &team_core::render::env_path(&compose.root, h.project, h.agent),
        env.as_bytes(),
    )
    .unwrap();
    let error = turn::run_turn(
        &compose,
        h,
        h.spec.execution.as_ref().unwrap(),
        None,
        "hello",
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("credential expired"));
    assert!(!error.contains("fixture-do-not-log"));
}

#[test]
fn docker_command_has_shared_network_explicit_mounts_and_no_implicit_credentials() {
    let (_dir, mut compose, _conn) = fixture();
    let execution = compose.projects[0]
        .managers
        .get_mut("a")
        .unwrap()
        .execution
        .as_mut()
        .unwrap();
    execution.sandbox.r#type = SandboxType::Docker;
    execution.sandbox.image = Some("teamctl-agent:test".into());
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    std::fs::write(compose.root.join(".env"), "NOT_FOR_CONTAINER=fixture").unwrap();
    std::fs::create_dir_all(compose.root.join("state/codex-home/other-agent")).unwrap();
    let args = sandbox::docker_args(
        &compose,
        h,
        h.spec.execution.as_ref().unwrap(),
        &BTreeMap::new(),
        "codex",
        &["exec".into()],
    )
    .unwrap();
    assert!(args.iter().any(|a| a == "--cap-drop=ALL"));
    assert!(args.iter().any(|a| a == "--security-opt=no-new-privileges"));
    assert!(args.iter().any(|a| a == &sandbox::network_name(&compose)));
    assert!(!args
        .iter()
        .any(|a| a.contains("docker.sock") || a.contains("auth.json")));
    assert!(args
        .iter()
        .any(|a| a.contains("source=/dev/null") && a.contains(".env,readonly")));
    assert!(args
        .iter()
        .any(|a| a.contains("type=tmpfs") && a.contains("other-agent")));
    assert_eq!(args.last().unwrap(), "exec");
    let (_, mcp) = team_core::render::render_agent(&compose, h, "/host/darwin/team-mcp");
    assert!(mcp.contains("/usr/local/bin/team-mcp"));
    assert!(!mcp.contains("darwin"));
    let codex =
        team_core::render::render_codex_config(&compose, h, "/host/darwin/team-mcp").unwrap();
    assert!(codex.contains("/usr/local/bin/team-mcp"));
}

#[test]
fn execution_validation_rejects_unbounded_wakes_and_socket_mounts() {
    let (_dir, mut compose, _conn) = fixture();
    let e = compose.projects[0]
        .managers
        .get_mut("a")
        .unwrap()
        .execution
        .as_mut()
        .unwrap();
    e.wake_interval_seconds = Some(1);
    let h = compose.agents().find(|h| h.id() == "p:a").unwrap();
    assert!(validate_execution(&compose, h).is_err());
    let errors = team_core::validate::validate(&compose);
    assert!(errors
        .iter()
        .any(|e| e.to_string().contains("wake_interval_seconds")));
}
