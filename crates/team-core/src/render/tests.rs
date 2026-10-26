use super::*;
use crate::compose::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn fixture() -> Compose {
    let mut managers = BTreeMap::new();
    managers.insert(
        "mgr".into(),
        Agent {
            execution: None,
            runtime: "claude-code".into(),
            model: Some("claude-opus-4-8".into()),
            role_prompt: Some(RolePrompt::Single(PathBuf::from("roles/mgr.md"))),
            permission_mode: Some("auto".into()),
            autonomy: "low_risk_only".into(),
            can_dm: vec![],
            can_broadcast: vec![],
            reports_to: None,
            on_rate_limit: None,
            effort: None,
            ultracode: false,
            interfaces: None,
            display_name: None,
            hooks: vec![],
            mcps: Default::default(),
            subagents: vec![],
            skills: vec![],
        },
    );
    Compose {
        root: PathBuf::from("/teamctl"),
        global: Global {
            web: Default::default(),
            version: crate::compose::SchemaVersion::new("2.0.0"),
            broker: Broker {
                r#type: "sqlite".into(),
                path: PathBuf::from("state/mailbox.db"),
            },
            supervisor: SupervisorCfg {
                r#type: "tmux".into(),
                tmux_prefix: "a-".into(),
                drain_timeout_secs: 10,
            },
            budget: Default::default(),
            hitl: Default::default(),
            rate_limits: Default::default(),
            interfaces: vec![],
            projects: vec![],
            attachments: Default::default(),
        },
        projects: vec![Project {
            version: 2,
            project: ProjectMeta {
                id: "hello".into(),
                name: "Hello".into(),
                cwd: PathBuf::from("/teamctl/examples/hello-team"),
            },
            channels: vec![],
            managers,
            workers: Default::default(),
            interfaces: None,
        }],
    }
}

#[test]
fn env_contains_agent_id_and_mailbox() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(env.contains("AGENT_ID=hello:mgr"));
    assert!(env.contains("TEAMCTL_MAILBOX=/teamctl/state/mailbox.db"));
    assert!(env.contains("TMUX_SESSION=a-hello-mgr"));
}

#[test]
fn env_emits_claude_session_id_and_name_for_claude_code_runtime() {
    // T-118: claude-code agents get deterministic UUIDv5 session
    // ids in their env so the wrapper can pass `--session-id` +
    // `-n` and resume the conversation across restarts.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let expected_id = crate::session::derive_session_id(h.project, h.agent);
    assert!(
        env.contains(&format!("CLAUDE_SESSION_ID={expected_id}\n")),
        "env was: {env}"
    );
    assert!(
        env.contains("CLAUDE_SESSION_NAME=teamctl:hello:mgr\n"),
        "env was: {env}"
    );
}

#[test]
fn env_omits_claude_session_vars_for_non_claude_runtimes() {
    // Other runtimes (codex, gemini) don't recognize claude's
    // `--session-id` flag — their wrapper arms must not see these
    // vars. Pin the gate so a future render refactor can't leak
    // them into every runtime.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        !env.contains("CLAUDE_SESSION_ID="),
        "non-claude runtime must not get session id: {env}"
    );
    assert!(
        !env.contains("CLAUDE_SESSION_NAME="),
        "non-claude runtime must not get session name: {env}"
    );
}

#[test]
fn env_pins_teamctl_root_to_compose_root() {
    // Regression: when project.cwd is a relative path (e.g. `..`),
    // the wrapper used to fall back to it for `--root`, which
    // resolves against the post-cd cwd and points at the wrong
    // directory. Rendering an absolute TEAMCTL_ROOT pins
    // `teamctl --root` to the compose root regardless of cwd.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(env.contains("TEAMCTL_ROOT=/teamctl\n"), "env was: {env}");
}

#[test]
fn env_omits_effort_when_unset() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(!env.contains("EFFORT="), "env was: {env}");
}

#[test]
fn env_emits_effort_when_set() {
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().effort = Some(EffortLevel::Max);
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(env.contains("EFFORT=max\n"), "env was: {env}");
}

#[test]
fn mcp_json_parses_back() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
    assert_eq!(
        v["mcpServers"]["team"]["command"],
        "/usr/local/bin/team-mcp"
    );
    assert_eq!(
        v["mcpServers"]["team"]["args"][1].as_str().unwrap(),
        "hello:mgr"
    );
}

#[test]
fn mcp_json_threads_tmux_prefix_from_compose() {
    // T-109: compact_self routes its tmux send-keys to
    // `<prefix><project>-<agent>` and reads the prefix from a CLI arg
    // (default `t-` only fits a stock team). Render must surface the
    // configured prefix so teams overriding it (e.g. `a-` here) get
    // their pane resolved correctly.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
    let args: Vec<&str> = v["mcpServers"]["team"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = args
        .iter()
        .position(|a| *a == "--tmux-prefix")
        .expect("render_mcp must emit --tmux-prefix so compact_self resolves the caller's pane");
    assert_eq!(
        args[i + 1],
        "a-",
        "prefix must come from compose, not the default"
    );
}

/// Build a `McpServer` test value tersely.
fn server(command: &str, args: &[&str]) -> McpServer {
    McpServer {
        url: None,
        bearer_token_env_var: None,
        env_http_headers: Default::default(),
        env_vars: vec![],
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: Default::default(),
    }
}

#[test]
fn mcp_json_includes_declared_servers_alongside_team() {
    // #383 Phase 4: a declared server lands in `mcpServers` next to
    // the built-in `team` server, with command/args/env passed
    // through verbatim (no `${VAR}` expansion in render).
    let mut c = fixture();
    let mut mcps = BTreeMap::new();
    let mut gh = server("npx", &["-y", "@modelcontextprotocol/server-github"]);
    gh.env
        .insert("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into());
    mcps.insert("github".into(), gh);
    c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;

    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();

    // Built-in team server survives untouched.
    assert_eq!(
        v["mcpServers"]["team"]["command"],
        "/usr/local/bin/team-mcp"
    );
    // Declared server present with verbatim fields.
    assert_eq!(v["mcpServers"]["github"]["command"], "npx");
    assert_eq!(v["mcpServers"]["github"]["args"][0], "-y");
    assert_eq!(
        v["mcpServers"]["github"]["env"]["GITHUB_TOKEN"], "${GITHUB_TOKEN}",
        "env values must pass through verbatim — claude expands ${{VAR}} at launch"
    );
    assert_eq!(v["mcpServers"].as_object().unwrap().len(), 2);
}

#[test]
fn mcp_json_team_server_is_non_clobberable() {
    // #383 Phase 4: a declared server literally named `team` must not
    // shadow the built-in mailbox bus — render skips it (validate also
    // rejects it). The `team` entry keeps the built-in command.
    let mut c = fixture();
    let mut mcps = BTreeMap::new();
    mcps.insert("team".into(), server("evil-team", &[]));
    mcps.insert("github".into(), server("npx", &[]));
    c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;

    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();

    assert_eq!(
        v["mcpServers"]["team"]["command"], "/usr/local/bin/team-mcp",
        "built-in team server must not be clobbered by a declared `team`"
    );
    assert!(v["mcpServers"]["github"].is_object());
    assert_eq!(
        v["mcpServers"].as_object().unwrap().len(),
        2,
        "the declared `team` is dropped, not added as a third entry"
    );
}

#[test]
fn mcp_json_unchanged_when_no_servers_declared() {
    // #383 Phase 4: empty `mcps` (the default) → only the built-in
    // team server, exactly as before this feature.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    assert!(servers.contains_key("team"));
}

#[test]
fn mcp_json_skips_declared_servers_on_runtime_without_mcp_support() {
    // #383 Phase 4: declared servers render only for runtimes whose
    // descriptor sets `supports_mcp`. A custom runtime that opts out
    // gets the team bus (unconditional) but not the declared servers.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("runtimes")).unwrap();
    std::fs::write(
        tmp.path().join("runtimes/codex.yaml"),
        "binary: codex\nsupports_mcp: false\n",
    )
    .unwrap();

    let mut c = fixture();
    c.root = tmp.path().to_path_buf();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "codex".into();
        let mut mcps = BTreeMap::new();
        mcps.insert("github".into(), server("npx", &[]));
        m.mcps = mcps;
    }

    let h = c.agents().next().unwrap();
    let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert!(servers.contains_key("team"), "team bus stays unconditional");
    assert!(
        !servers.contains_key("github"),
        "declared server skipped when runtime lacks supports_mcp"
    );
    assert_eq!(servers.len(), 1);
}

#[test]
fn env_emits_codex_home_for_codex_runtime() {
    // Codex has no --mcp-config flag; the wrapper's codex arm reads
    // CODEX_HOME from the env file and exports it so codex finds the
    // rendered [mcp_servers.*] config.toml.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        env.contains("CODEX_HOME=/teamctl/state/codex-home/hello-mgr\n"),
        "env was: {env}"
    );
}

#[test]
fn env_omits_codex_home_for_non_codex_runtimes() {
    // Only the codex arm consumes CODEX_HOME; leaking it into other
    // runtimes' envs would relocate their state if a same-named knob
    // ever appears. Pin the gate for claude-code and gemini both.
    for runtime in ["claude-code", "gemini"] {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = runtime.into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            !env.contains("CODEX_HOME="),
            "{runtime} must not get CODEX_HOME: {env}"
        );
    }
}

#[test]
fn codex_config_present_with_team_server_for_codex_runtime() {
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp")
        .expect("codex agent must get a config.toml");
    assert!(toml.contains("[mcp_servers.team]"), "toml was: {toml}");
    assert!(
        toml.contains("command = \"/usr/local/bin/team-mcp\""),
        "toml was: {toml}"
    );
    // Same args as the JSON transport — the shared team_server_args
    // helper is the single source of truth.
    assert!(
        toml.contains("\"--agent-id\", \"hello:mgr\""),
        "toml was: {toml}"
    );
    assert!(
        toml.contains("\"--tmux-prefix\", \"a-\""),
        "toml was: {toml}"
    );
}

#[test]
fn codex_config_absent_for_non_codex_runtimes() {
    // claude/gemini get their MCP servers via the JSON file; a stray
    // config.toml would be dead weight on disk.
    let c = fixture();
    let h = c.agents().next().unwrap();
    assert!(render_codex_config(&c, h, "/usr/local/bin/team-mcp").is_none());
}

#[test]
fn codex_config_preapproves_team_tools_and_trusts_cwd() {
    // Codex prompts per MCP tool even under `-a never`, and persists
    // human answers INTO config.toml — which this render rewrites on
    // every up/reload. Without these seeded tables an unattended
    // codex pane strands on its first inbox_peek (observed live on
    // codex-cli 0.144.3). Shapes verbatim-match what codex itself
    // writes.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    for tool in super::CODEX_PREAPPROVED_TEAM_TOOLS {
        assert!(
            toml.contains(&format!("[mcp_servers.team.tools.{tool}]")),
            "missing pre-approval table for {tool}: {toml}"
        );
    }
    assert!(
        toml.contains("approval_mode = \"approve\""),
        "toml was: {toml}"
    );
    assert!(toml.contains("[projects."), "toml was: {toml}");
    assert!(
        toml.contains("trust_level = \"trusted\""),
        "toml was: {toml}"
    );
    // Network is on inside the workspace-write sandbox so agents use
    // plain git/gh instead of connector apps that would prompt.
    assert!(
        toml.contains("[sandbox_workspace_write]\nnetwork_access = true"),
        "toml must enable sandbox network: {toml}"
    );
}

#[test]
fn codex_config_includes_declared_servers() {
    // Declared `mcps:` land as their own [mcp_servers.<name>] tables
    // next to the team bus, mirroring the JSON transport's merge.
    // The `${GITHUB_TOKEN}` placeholder passes through literally on
    // purpose (expanding it here would write the resolved secret to
    // disk) — but unlike claude, codex does NOT interpolate it at
    // launch, so validate warns (McpEnvInterpolationUnsupported).
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "codex".into();
        let mut gh = server("npx", &["-y", "@modelcontextprotocol/server-github"]);
        gh.env
            .insert("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into());
        let mut mcps = BTreeMap::new();
        mcps.insert("github".into(), gh);
        m.mcps = mcps;
    }
    let h = c.agents().next().unwrap();
    let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(toml.contains("[mcp_servers.team]"), "toml was: {toml}");
    assert!(toml.contains("[mcp_servers.github]"), "toml was: {toml}");
    assert!(toml.contains("command = \"npx\""), "toml was: {toml}");
    assert!(
        toml.contains("args = [\"-y\", \"@modelcontextprotocol/server-github\"]"),
        "toml was: {toml}"
    );
    assert!(
        toml.contains("[mcp_servers.github.env]\nGITHUB_TOKEN = \"${GITHUB_TOKEN}\""),
        "env must land in a sub-table, verbatim — codex won't expand it: {toml}"
    );
}

#[test]
fn codex_config_escapes_toml_strings() {
    // A quote or backslash in an env value must not break the TOML —
    // basic strings escape both (team-core hand-renders, no toml crate).
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "codex".into();
        let mut srv = server("run", &[]);
        srv.env
            .insert("TRICKY".into(), "say \"hi\" C:\\path".into());
        let mut mcps = BTreeMap::new();
        mcps.insert("x".into(), srv);
        m.mcps = mcps;
    }
    let h = c.agents().next().unwrap();
    let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(
        toml.contains("TRICKY = \"say \\\"hi\\\" C:\\\\path\""),
        "toml was: {toml}"
    );
}

#[test]
fn write_codex_config_writes_then_clears_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = fixture();
    c.root = dir.path().to_path_buf();
    let dest = codex_home_dir(&c.root, "hello", "mgr").join("config.toml");

    // codex runtime → config materialized under the per-agent home.
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    write_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(dest.exists(), "codex config.toml should be written");

    // Runtime switched away → the managed config is removed (the home
    // dir itself survives: it may hold codex session history).
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "claude-code".into();
    let h = c.agents().next().unwrap();
    write_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(!dest.exists(), "stale codex config.toml should be removed");
}

#[test]
fn env_emits_opencode_vars_for_opencode_runtime() {
    // The wrapper's opencode arm reads OPENCODE_DB (per-agent
    // session sqlite db — the resume-probe target) and
    // OPENCODE_CONFIG (per-agent json carrying the MCP servers +
    // instructions) from the env file.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "opencode".into();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        env.contains("OPENCODE_DB=/teamctl/state/opencode-home/hello-mgr/agent.db\n"),
        "env was: {env}"
    );
    assert!(
        env.contains("OPENCODE_CONFIG=/teamctl/state/opencode-home/hello-mgr/opencode.json\n"),
        "env was: {env}"
    );
}

#[test]
fn env_omits_opencode_vars_for_non_opencode_runtimes() {
    // Only the opencode arm consumes these; leaking them into other
    // runtimes' envs would relocate their state if a same-named
    // knob ever appears. Pin the gate for every other runtime.
    for runtime in ["claude-code", "codex", "gemini"] {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = runtime.into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            !env.contains("OPENCODE_DB=") && !env.contains("OPENCODE_CONFIG="),
            "{runtime} must not get opencode vars: {env}"
        );
    }
}

#[test]
fn opencode_config_present_with_team_server_for_opencode_runtime() {
    // Pin the EXACT mcp entry key names (`type`/`command`/
    // `environment`/`enabled`): opencode silently drops entries
    // that violate its schema (verified — renaming `command` made
    // the server vanish with no error), so a drifted key here would
    // sever the mailbox with zero diagnostics.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "opencode".into();
    let h = c.agents().next().unwrap();
    let json = render_opencode_config(&c, h, "/usr/local/bin/team-mcp")
        .expect("opencode agent must get an opencode.json");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let team = &v["mcp"]["team"];
    assert_eq!(team["type"], "local");
    assert_eq!(team["enabled"], true);
    // Single argv array (binary first) — same args as the JSON
    // transport, from the shared team_server_args helper.
    assert_eq!(team["command"][0], "/usr/local/bin/team-mcp");
    assert_eq!(team["command"][1], "--agent-id");
    assert_eq!(team["command"][2], "hello:mgr");
    let argv: Vec<&str> = team["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = argv.iter().position(|a| *a == "--tmux-prefix").unwrap();
    assert_eq!(argv[i + 1], "a-");
    // The opt-outs the wrapper depends on.
    assert_eq!(v["autoupdate"], false);
    assert_eq!(v["share"], "disabled");
    // Role prompt rides `instructions` as an absolute path.
    assert_eq!(
        v["instructions"],
        serde_json::json!(["/teamctl/roles/mgr.md"])
    );
}

#[test]
fn opencode_config_absent_for_non_opencode_runtimes() {
    // Other runtimes get their MCP servers via the JSON file (or
    // the codex TOML); a stray opencode.json would be dead weight.
    let c = fixture();
    let h = c.agents().next().unwrap();
    assert!(render_opencode_config(&c, h, "/usr/local/bin/team-mcp").is_none());
}

#[test]
fn opencode_config_omits_instructions_without_role_prompt() {
    // No role_prompt → no `instructions` key at all (an empty list
    // or a blank path would be schema noise opencode may silently
    // choke on).
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "opencode".into();
        m.role_prompt = None;
    }
    let h = c.agents().next().unwrap();
    let json = render_opencode_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("instructions").is_none(), "json was: {json}");
}

#[test]
fn opencode_config_includes_declared_servers() {
    // Declared `mcps:` land as their own `mcp` entries next to the
    // team bus, mirroring the JSON transport's merge. env values
    // pass through verbatim under the exact `environment` key.
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "opencode".into();
        let mut gh = server("npx", &["-y", "@modelcontextprotocol/server-github"]);
        gh.env
            .insert("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into());
        let mut mcps = BTreeMap::new();
        mcps.insert("github".into(), gh);
        m.mcps = mcps;
    }
    let h = c.agents().next().unwrap();
    let json = render_opencode_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let gh = &v["mcp"]["github"];
    assert_eq!(gh["type"], "local");
    assert_eq!(gh["enabled"], true);
    assert_eq!(
        gh["command"],
        serde_json::json!(["npx", "-y", "@modelcontextprotocol/server-github"])
    );
    assert_eq!(
        gh["environment"]["GITHUB_TOKEN"], "${GITHUB_TOKEN}",
        "env values must pass through verbatim under `environment`"
    );
    assert_eq!(v["mcp"].as_object().unwrap().len(), 2);
}

#[test]
fn opencode_config_team_server_is_non_clobberable() {
    // Same guarantee as the JSON + TOML transports: a declared
    // server literally named `team` must not shadow the mailbox bus.
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "opencode".into();
        let mut mcps = BTreeMap::new();
        mcps.insert("team".into(), server("evil-team", &[]));
        m.mcps = mcps;
    }
    let h = c.agents().next().unwrap();
    let json = render_opencode_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["mcp"]["team"]["command"][0], "/usr/local/bin/team-mcp",
        "built-in team server must not be clobbered by a declared `team`"
    );
    assert_eq!(v["mcp"].as_object().unwrap().len(), 1);
}

#[test]
fn write_opencode_config_writes_then_clears_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = fixture();
    c.root = dir.path().to_path_buf();
    let dest = opencode_home_dir(&c.root, "hello", "mgr").join("opencode.json");

    // opencode runtime → config materialized under the per-agent home.
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "opencode".into();
    let h = c.agents().next().unwrap();
    write_opencode_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(dest.exists(), "opencode.json should be written");

    // Runtime switched away → the managed config is removed (the home
    // dir itself survives: it holds the agent's session db).
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "claude-code".into();
    let h = c.agents().next().unwrap();
    write_opencode_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
    assert!(!dest.exists(), "stale opencode.json should be removed");
}

#[test]
fn env_points_at_source_for_single_role_prompt() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        env.contains("SYSTEM_PROMPT_PATH=/teamctl/roles/mgr.md\n"),
        "env was: {env}"
    );
}

#[test]
fn env_points_at_concat_path_for_multi_role_prompt() {
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(RolePrompt::Multiple(vec![
        PathBuf::from("roles/_base.md"),
        PathBuf::from("roles/mgr.md"),
    ]));
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        env.contains("SYSTEM_PROMPT_PATH=/teamctl/state/role_prompts/hello-mgr.md\n"),
        "env was: {env}"
    );
}

#[test]
fn write_role_prompt_concat_is_noop_for_single() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = fixture();
    c.root = dir.path().to_path_buf();
    let h = c.agents().next().unwrap();
    write_role_prompt_concat(&c, h).unwrap();
    assert!(
        !role_prompt_concat_path(&c.root, h.project, h.agent).exists(),
        "single-form role_prompt should not produce a concat file"
    );
}

#[test]
fn write_role_prompt_concat_joins_in_declared_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("roles")).unwrap();
    std::fs::write(root.join("roles/_base.md"), "BASE").unwrap();
    std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

    let mut c = fixture();
    c.root = root.to_path_buf();
    c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(RolePrompt::Multiple(vec![
        PathBuf::from("roles/_base.md"),
        PathBuf::from("roles/mgr.md"),
    ]));
    let h = c.agents().next().unwrap();
    write_role_prompt_concat(&c, h).unwrap();

    let dest = role_prompt_concat_path(root, h.project, h.agent);
    let got = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(got, "BASE\n\n—\n\nMGR");
}

#[test]
fn write_role_prompt_concat_reflects_source_edits() {
    // Owner-flagged: editing a source file must show up at the next
    // render. We re-write unconditionally rather than caching.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("roles")).unwrap();
    std::fs::write(root.join("roles/_base.md"), "v1").unwrap();
    std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

    let mut c = fixture();
    c.root = root.to_path_buf();
    c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(RolePrompt::Multiple(vec![
        PathBuf::from("roles/_base.md"),
        PathBuf::from("roles/mgr.md"),
    ]));
    let h = c.agents().next().unwrap();
    write_role_prompt_concat(&c, h).unwrap();

    std::fs::write(root.join("roles/_base.md"), "v2").unwrap();
    let h = c.agents().next().unwrap();
    write_role_prompt_concat(&c, h).unwrap();

    let dest = role_prompt_concat_path(root, h.project, h.agent);
    let got = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(got, "v2\n\n—\n\nMGR");
}

#[test]
fn claude_settings_present_for_claude_code() {
    // T-189: claude-code agents get a wrapper-managed settings
    // file with a PreToolUse deny hook for synchronous-prompt
    // tools that would otherwise strand a headless pane.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    let pre = &v["hooks"]["PreToolUse"][0];
    assert_eq!(
        pre["matcher"].as_str().unwrap(),
        "AskUserQuestion|EnterPlanMode|ExitPlanMode"
    );
    let cmd = pre["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains(r#""permissionDecision":"deny""#),
        "deny verdict missing from hook command: {cmd}"
    );
    assert!(
        cmd.contains("Interactive prompts are disabled"),
        "systemMessage missing from hook command: {cmd}"
    );
}

#[test]
fn claude_settings_pre_trust_all_project_mcp_servers() {
    // #421: the rendered settings carry `enableAllProjectMcpServers: true`
    // at the top level so a headless agent never freezes on Claude's "New
    // MCP server found in this project" prompt (no human to confirm it).
    // Attended sessions skip `--settings` entirely, so this only affects
    // unattended panes; non-claude runtimes get no settings file at all
    // (covered by `claude_settings_absent_for_non_claude_runtimes`).
    let c = fixture();
    let h = c.agents().next().unwrap();
    let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(
        v["enableAllProjectMcpServers"],
        serde_json::Value::Bool(true),
        "headless settings must pre-trust project MCP servers: {s}"
    );
}

#[test]
fn claude_settings_absent_for_non_claude_runtimes() {
    // codex/gemini don't read claude settings; the file would be
    // dead weight and a confusing artifact on disk.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    assert!(render_claude_settings(&c, h).is_none());
}

#[test]
fn claude_settings_absent_when_non_claude_agent_opts_into_ultracode() {
    // #461: a declared `ultracode: true` on a non-claude runtime is a
    // no-op — the whole settings file is skipped, so the opt-in must
    // never leak into a rendered artifact. Pins that the early-return
    // None survives even with the opt-in set (the warn fires; the
    // contract is the None).
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "codex".into();
        m.ultracode = true;
    }
    let h = c.agents().next().unwrap();
    assert!(render_claude_settings(&c, h).is_none());
}

#[test]
fn declared_hook_merges_alongside_deny_hook() {
    // #383 Phase 2 + #428: a per-agent PreToolUse hook is appended
    // AFTER the built-ins in the same bucket — the deny hook keeps slot
    // 0, the #428 heartbeat touch sits at slot 1, and the declared hook
    // lands at slot 2 with its command resolved to an absolute path.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().hooks = vec![HookSpec {
        event: "PreToolUse".into(),
        matcher: Some("Bash".into()),
        command: PathBuf::from("hooks/guard.sh"),
    }];
    let h = c.agents().next().unwrap();
    let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        pre.len(),
        3,
        "deny hook + #428 heartbeat touch + declared hook expected"
    );
    // Built-in deny hook survives in slot 0.
    assert_eq!(
        pre[0]["matcher"].as_str().unwrap(),
        "AskUserQuestion|EnterPlanMode|ExitPlanMode"
    );
    assert!(pre[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains(r#""permissionDecision":"deny""#));
    // #428 heartbeat touch at slot 1 (match-all, no matcher).
    assert!(
        pre[1].get("matcher").is_none(),
        "heartbeat touch must be match-all: {}",
        pre[1]
    );
    // Declared hook appended after the built-ins.
    assert_eq!(pre[2]["matcher"].as_str().unwrap(), "Bash");
    assert_eq!(pre[2]["hooks"][0]["type"].as_str().unwrap(), "command");
    assert_eq!(
        pre[2]["hooks"][0]["command"].as_str().unwrap(),
        "/teamctl/hooks/guard.sh"
    );
}

#[test]
fn claude_settings_emits_ultracode_when_set() {
    // #461: opting an agent into ultracode renders `"ultracode": true`
    // into its Claude Code settings JSON — the file the wrapper passes
    // via `--settings`.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().ultracode = true;
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    assert_eq!(v["ultracode"], true);
}

#[test]
fn claude_settings_omits_ultracode_when_unset() {
    // #461: with ultracode left at its default (`false`), the key is
    // absent entirely — not present-and-false — so the settings shape is
    // byte-identical for agents that don't opt in.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    assert!(v.get("ultracode").is_none());
}

#[test]
fn default_hooks_are_deny_plus_heartbeat_buckets() {
    // #383 Phase 2 + #428 + #430 + #431: with no compose-declared hooks,
    // the settings file renders exactly the built-in default buckets: the
    // `PreToolUse` deny hook, the #428 activity-heartbeat hooks, the #430
    // `SessionStart` boot-context hook, and the #431 `StopFailure`
    // rate-limit marker, and nothing else. Asserted as an exact key-set
    // (not a raw count) so each future built-in extends the set
    // deterministically instead of racing on a number.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    let hooks = v["hooks"].as_object().unwrap();
    let keys: std::collections::BTreeSet<&str> = hooks.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "PreToolUse",
            "SessionStart",
            "Stop",
            "StopFailure",
            "UserPromptSubmit"
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>(),
        "exact set of built-in default hook buckets expected with no declared hooks"
    );
    // PreToolUse holds the deny hook (slot 0) + the heartbeat touch.
    assert_eq!(
        hooks["PreToolUse"].as_array().unwrap().len(),
        2,
        "deny hook + heartbeat touch expected"
    );
    // StopFailure holds the heartbeat clear (slot 0) + the #431 rate-limit
    // marker (slot 1): slot 0 is the match-all `rm -f`, slot 1 carries the
    // `rate_limit` matcher and the `rl-hit` command.
    let stop_failure = hooks["StopFailure"].as_array().unwrap();
    assert_eq!(
        stop_failure.len(),
        2,
        "heartbeat clear + rate-limit marker expected"
    );
    assert!(
        stop_failure[0].get("matcher").is_none(),
        "StopFailure slot 0 should be the match-all heartbeat clear"
    );
    // #439: the heartbeat clear now records LASTSEEN before removing the
    // marker, so the command leads with `touch ` and still rm's the marker.
    let clear_cmd = stop_failure[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(
        clear_cmd.starts_with("touch ") && clear_cmd.contains(" && rm -f "),
        "StopFailure slot 0 should touch lastseen then rm the marker: {clear_cmd}"
    );
    assert_eq!(
        stop_failure[1]["matcher"].as_str().unwrap(),
        "rate_limit",
        "StopFailure slot 1 should scope to rate-limit stops"
    );
    assert!(stop_failure[1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("rl-hit"));
    // Stop holds the heartbeat clear (slot 0) + the #333 budget cost writer
    // (slot 1): both are match-all (no matcher), slot 1 carries the
    // `budget-record` command.
    let stop = hooks["Stop"].as_array().unwrap();
    assert_eq!(
        stop.len(),
        2,
        "heartbeat clear + budget cost writer expected"
    );
    assert!(
        stop[0].get("matcher").is_none(),
        "Stop slot 0 should be the match-all heartbeat clear"
    );
    let stop_clear = stop[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(
        stop_clear.starts_with("touch ") && stop_clear.contains(" && rm -f "),
        "Stop slot 0 should touch lastseen then rm the marker: {stop_clear}"
    );
    assert!(
        stop[1].get("matcher").is_none(),
        "Stop slot 1 (budget writer) should be match-all"
    );
    assert!(stop[1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("budget-record"));
    // Each remaining single-entry built-in bucket holds exactly its one entry.
    for ev in ["UserPromptSubmit", "SessionStart"] {
        assert_eq!(
            hooks[ev].as_array().unwrap().len(),
            1,
            "{ev} should hold exactly one built-in entry"
        );
    }
}

#[test]
fn stop_failure_rate_limit_hook_records_a_hit() {
    // #431: the StopFailure bucket's slot-1 entry is the rate-limit marker.
    // The canary `default_hooks_are_deny_plus_heartbeat_buckets` pins the
    // bucket shape (2 entries, slot 1 matcher `rate_limit` + `rl-hit`); this
    // pins the load-bearing details of the emitted command string.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    let stop_failure = v["hooks"]["StopFailure"].as_array().unwrap();
    let command = stop_failure[1]["hooks"][0]["command"].as_str().unwrap();

    // Guard first so a host without `teamctl` on PATH never errors the stop.
    assert!(
        command.starts_with("command -v teamctl >/dev/null"),
        "rl-hit command must lead with the PATH guard: {command}"
    );
    // The compose root is baked in so the hook needs no env to find the db.
    assert!(
        command.contains("--root"),
        "rl-hit command must pass the compose --root: {command}"
    );
    // The subcommand and the agent's `<project>:<agent>` id, pulled from the
    // fixture handle so the assertion tracks the fixture rather than a
    // hard-coded literal.
    assert!(
        command.contains("rl-hit"),
        "rl-hit subcommand missing: {command}"
    );
    let agent_id = format!("{}:{}", h.project, h.agent);
    assert!(
        command.contains(&agent_id),
        "rl-hit command must target the agent id {agent_id}: {command}"
    );
    // Trailing `|| true` makes the marker pure fire-and-forget: any rl-hit
    // error degrades to a silent exit-0 instead of erroring the stop.
    assert!(
        command.ends_with("|| true"),
        "rl-hit command must end with the fire-and-forget guard: {command}"
    );
}

#[test]
fn stop_budget_record_hook_records_cost() {
    // #333: the Stop bucket's slot-1 entry is the budget cost writer. The
    // canary `default_hooks_are_deny_plus_heartbeat_buckets` pins the bucket
    // shape (2 entries, slot 1 match-all + `budget-record`); this pins the
    // load-bearing details of the emitted command string, mirroring the #431
    // rl-hit canary exactly.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    let stop = v["hooks"]["Stop"].as_array().unwrap();
    let command = stop[1]["hooks"][0]["command"].as_str().unwrap();

    // Guard first so a host without `teamctl` on PATH never errors the stop.
    assert!(
        command.starts_with("command -v teamctl >/dev/null"),
        "budget-record command must lead with the PATH guard: {command}"
    );
    // The compose root is baked in so the hook needs no env to find the db.
    assert!(
        command.contains("--root"),
        "budget-record command must pass the compose --root: {command}"
    );
    // The subcommand and the agent's `<project>:<agent>` id, pulled from the
    // fixture handle so the assertion tracks the fixture rather than a
    // hard-coded literal.
    assert!(
        command.contains("budget-record"),
        "budget-record subcommand missing: {command}"
    );
    let agent_id = format!("{}:{}", h.project, h.agent);
    assert!(
        command.contains(&agent_id),
        "budget-record command must target the agent id {agent_id}: {command}"
    );
    // Trailing `|| true` makes the writer pure fire-and-forget: any record
    // error degrades to a silent exit-0 instead of erroring the stop.
    assert!(
        command.ends_with("|| true"),
        "budget-record command must end with the fire-and-forget guard: {command}"
    );
}

#[test]
fn heartbeat_hooks_touch_and_clear_the_marker() {
    // #428: the four activity-heartbeat hooks render with the agent's
    // marker path, `type:command`, and no `matcher` (match-all). touch
    // on PreToolUse/UserPromptSubmit, rm on Stop/StopFailure.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let path = heartbeat_path(&c.root, h.project, h.agent)
        .display()
        .to_string();
    // Same shlex quoting the renderer uses — pins the exact emitted
    // command, so a regression in quoting (or a dropped `touch`/`rm`)
    // fails here rather than silently misfiring at runtime.
    let q = crate::supervisor::shlex::try_quote(&path).unwrap();
    // #439: the turn-end clear also touches the LASTSEEN sibling.
    let ls_path = lastseen_path(&c.root, h.project, h.agent)
        .display()
        .to_string();
    let ls = crate::supervisor::shlex::try_quote(&ls_path).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    let hooks = &v["hooks"];

    // PreToolUse: deny stays at slot 0, heartbeat touch appended at slot 1.
    let touch_entry = &hooks["PreToolUse"].as_array().unwrap()[1];
    assert!(
        touch_entry.get("matcher").is_none(),
        "heartbeat must be match-all (no matcher): {touch_entry}"
    );
    assert_eq!(touch_entry["hooks"][0]["type"].as_str().unwrap(), "command");
    assert_eq!(
        touch_entry["hooks"][0]["command"].as_str().unwrap(),
        format!("touch {q}"),
        "PreToolUse should touch the quoted marker"
    );

    // UserPromptSubmit touches the same marker.
    assert_eq!(
        hooks["UserPromptSubmit"].as_array().unwrap()[0]["hooks"][0]["command"]
            .as_str()
            .unwrap(),
        format!("touch {q}"),
        "UserPromptSubmit should touch the quoted marker"
    );

    // Stop + StopFailure clear it. The heartbeat clear is always slot 0
    // (match-all); on StopFailure the #431 rate-limit marker follows at
    // slot 1, so the heartbeat entry keeps its slot. #439: the clear first
    // touches the LASTSEEN sibling, then rm's the marker, in one command.
    for ev in ["Stop", "StopFailure"] {
        let entry = &hooks[ev].as_array().unwrap()[0];
        assert!(
            entry.get("matcher").is_none(),
            "{ev} must be match-all (no matcher)"
        );
        assert_eq!(
            entry["hooks"][0]["command"].as_str().unwrap(),
            format!("touch {ls} && rm -f {q}"),
            "{ev} should touch the quoted lastseen then rm the quoted marker"
        );
    }
}

#[test]
fn session_start_hook_runs_the_boot_script() {
    // #430: the SessionStart boot-context hook renders as a single
    // match-all entry whose command is the shlex-quoted absolute path to
    // the shared `bin/boot.sh` asset, with a 5s timeout. #439: the command
    // now passes two positional argv — the quoted LASTSEEN then MARKER
    // paths — so the script can compute downtime. The script (not this
    // JSON) emits the REQUIRED `hookEventName` — here we pin the wiring
    // that points Claude Code at it, that it carries the per-agent argv,
    // and that it fires on every source (no matcher).
    let c = fixture();
    let h = c.agents().next().unwrap();
    let path = boot_script_path(&c.root).display().to_string();
    let q = crate::supervisor::shlex::try_quote(&path).unwrap();
    let ls_path = lastseen_path(&c.root, h.project, h.agent)
        .display()
        .to_string();
    let ls = crate::supervisor::shlex::try_quote(&ls_path).unwrap();
    let marker_path = heartbeat_path(&c.root, h.project, h.agent)
        .display()
        .to_string();
    let marker = crate::supervisor::shlex::try_quote(&marker_path).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    let bucket = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(
        bucket.len(),
        1,
        "exactly one SessionStart built-in expected"
    );
    let entry = &bucket[0];
    assert!(
        entry.get("matcher").is_none(),
        "SessionStart must be match-all (fire on every source): {entry}"
    );
    let inner = &entry["hooks"][0];
    assert_eq!(inner["type"].as_str().unwrap(), "command");
    assert_eq!(
        inner["command"].as_str().unwrap(),
        format!("{q} {ls} {marker}"),
        "SessionStart should run the quoted boot.sh path with lastseen + marker argv"
    );
    assert_eq!(inner["timeout"].as_i64().unwrap(), 5, "5s timeout expected");
}

#[test]
fn lastseen_path_is_a_dotlastseen_sibling_of_the_marker() {
    // #439: the lastseen marker lives in the same state/heartbeats dir as
    // the heartbeat marker, with the same `<project>-<agent>` stem plus a
    // `.lastseen` suffix — so it never shadows the marker the TUI stats for
    // Working/Idle. The two render tests use this fn on both sides of their
    // assertions; this pins the literal shape they rely on.
    let root = std::path::Path::new("/srv/.team");
    let marker = heartbeat_path(root, "proj", "ada");
    let lastseen = lastseen_path(root, "proj", "ada");
    assert_eq!(lastseen, root.join("state/heartbeats/proj-ada.lastseen"));
    assert_eq!(
        lastseen.parent(),
        marker.parent(),
        "lastseen must sit in the same heartbeats dir as the marker"
    );
    assert_ne!(
        lastseen, marker,
        "lastseen must not collide with the marker"
    );
}

#[test]
fn declared_hook_without_matcher_opens_new_event_bucket() {
    // #383 Phase 2: a hook on a fresh event (no matcher) creates its
    // own bucket and omits `matcher` so Claude Code matches all tools;
    // PreToolUse keeps only its built-ins (deny + #428 heartbeat) since
    // this declared hook targets PostToolUse.
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().hooks = vec![HookSpec {
        event: "PostToolUse".into(),
        matcher: None,
        command: PathBuf::from("hooks/log.sh"),
    }];
    let h = c.agents().next().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
    assert_eq!(
        v["hooks"]["PreToolUse"].as_array().unwrap().len(),
        2,
        "PreToolUse keeps its deny + #428 heartbeat built-ins"
    );
    let post = &v["hooks"]["PostToolUse"].as_array().unwrap()[0];
    assert!(
        post.get("matcher").is_none(),
        "matcher must be omitted when unset: {post}"
    );
    assert_eq!(
        post["hooks"][0]["command"].as_str().unwrap(),
        "/teamctl/hooks/log.sh"
    );
}

#[test]
fn declared_hooks_noop_on_non_claude_runtime() {
    // #383 Phase 2: hooks are claude-only v1 — declared on codex the
    // whole settings file is still skipped (render warns, returns None).
    let mut c = fixture();
    {
        let m = c.projects[0].managers.get_mut("mgr").unwrap();
        m.runtime = "codex".into();
        m.hooks = vec![HookSpec {
            event: "PreToolUse".into(),
            matcher: Some("Bash".into()),
            command: PathBuf::from("hooks/guard.sh"),
        }];
    }
    let h = c.agents().next().unwrap();
    assert!(
        render_claude_settings(&c, h).is_none(),
        "hooks must not render on non-claude runtimes"
    );
}

#[test]
fn env_emits_claude_settings_path_for_claude_code() {
    // T-189: wrapper reads CLAUDE_SETTINGS and passes it to claude
    // via `--settings`. Path must resolve under the compose root.
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        env.contains("CLAUDE_SETTINGS=/teamctl/state/claude/hello-mgr.json\n"),
        "env was: {env}"
    );
}

#[test]
fn env_omits_claude_settings_for_non_claude_runtimes() {
    // Only claude-code reads the settings file; other runtimes
    // must not see the env var (avoids confusion if they ever add
    // a same-named knob).
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        !env.contains("CLAUDE_SETTINGS="),
        "non-claude runtime must not get settings path: {env}"
    );
}

#[test]
fn write_role_prompt_concat_errors_on_missing_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = fixture();
    c.root = dir.path().to_path_buf();
    c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
        Some(RolePrompt::Multiple(vec![PathBuf::from(
            "roles/missing.md",
        )]));
    let h = c.agents().next().unwrap();
    let err = write_role_prompt_concat(&c, h).unwrap_err();
    assert!(err.to_string().contains("missing.md"), "err was: {err}");
}

// ---- #383 Phase 3a: per-agent sub-agents (`--agents` JSON) ----

fn write_file(root: &std::path::Path, rel: &str, contents: &str) {
    let abs = root.join(rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(abs, contents).unwrap();
}

fn rooted(write: impl FnOnce(&std::path::Path)) -> (tempfile::TempDir, Compose) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = fixture();
    c.root = dir.path().to_path_buf();
    write(dir.path());
    (dir, c)
}

#[test]
fn render_subagents_builds_agents_json_from_frontmatter() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/security-auditor.md",
            "---\nname: security-auditor\ndescription: Audits diffs for vulns.\n\
                 tools: Read, Grep\nmodel: claude-sonnet-4-6\n---\n\
                 You are a security auditor.\nFlag risky patterns.\n",
        );
    });
    c.projects[0].managers.get_mut("mgr").unwrap().subagents =
        vec![PathBuf::from("agents/security-auditor.md")];
    let h = c.agents().next().unwrap();
    let json = render_subagents(&c, h).unwrap().expect("some json");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entry = &v["security-auditor"];
    assert_eq!(entry["description"], "Audits diffs for vulns.");
    assert_eq!(
        entry["prompt"],
        "You are a security auditor.\nFlag risky patterns."
    );
    assert_eq!(entry["tools"], serde_json::json!(["Read", "Grep"]));
    assert_eq!(entry["model"], "claude-sonnet-4-6");
}

#[test]
fn render_subagents_name_falls_back_to_file_stem() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/repo-cartographer.md",
            "---\ndescription: Maps the repo.\n---\nMap it.\n",
        );
    });
    c.projects[0].managers.get_mut("mgr").unwrap().subagents =
        vec![PathBuf::from("agents/repo-cartographer.md")];
    let h = c.agents().next().unwrap();
    let json = render_subagents(&c, h).unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        v.get("repo-cartographer").is_some(),
        "stem-derived name missing: {json}"
    );
    // Nothing declared beyond description → optional keys omitted.
    assert!(v["repo-cartographer"].get("tools").is_none());
    assert!(v["repo-cartographer"].get("model").is_none());
}

#[test]
fn render_subagents_supports_yaml_list_tools() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/x.md",
            "---\nname: x\ndescription: d\ntools: [Read, Bash]\n---\nbody\n",
        );
    });
    c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![PathBuf::from("agents/x.md")];
    let h = c.agents().next().unwrap();
    let json = render_subagents(&c, h).unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["x"]["tools"], serde_json::json!(["Read", "Bash"]));
}

#[test]
fn render_subagents_isolates_per_agent() {
    // Two agents declaring different sub-agents must each get only
    // their own — the core per-agent-scope guarantee.
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/a.md",
            "---\nname: a\ndescription: da\n---\nba\n",
        );
        write_file(
            root,
            "agents/b.md",
            "---\nname: b\ndescription: db\n---\nbb\n",
        );
    });
    let worker = c.projects[0].managers["mgr"].clone();
    c.projects[0].workers.insert("dev".into(), worker);
    c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![PathBuf::from("agents/a.md")];
    c.projects[0].workers.get_mut("dev").unwrap().subagents = vec![PathBuf::from("agents/b.md")];

    for h in c.agents() {
        let v: serde_json::Value =
            serde_json::from_str(&render_subagents(&c, h).unwrap().unwrap()).unwrap();
        match h.agent {
            "mgr" => {
                assert!(v.get("a").is_some() && v.get("b").is_none());
            }
            "dev" => {
                assert!(v.get("b").is_some() && v.get("a").is_none());
            }
            other => panic!("unexpected agent {other}"),
        }
    }
}

#[test]
fn render_subagents_none_when_empty() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    assert!(render_subagents(&c, h).unwrap().is_none());
}

#[test]
fn render_subagents_ignored_on_non_claude_runtime() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/x.md",
            "---\nname: x\ndescription: d\n---\nb\n",
        );
    });
    {
        let a = c.projects[0].managers.get_mut("mgr").unwrap();
        a.runtime = "codex".into();
        a.subagents = vec![PathBuf::from("agents/x.md")];
    }
    let h = c.agents().next().unwrap();
    // claude-only v1: codex ignores declared sub-agents (warns).
    assert!(render_subagents(&c, h).unwrap().is_none());
}

#[test]
fn render_subagents_errors_on_missing_source() {
    let (_d, mut c) = rooted(|_| {});
    c.projects[0].managers.get_mut("mgr").unwrap().subagents =
        vec![PathBuf::from("agents/nope.md")];
    let h = c.agents().next().unwrap();
    let err = render_subagents(&c, h).unwrap_err();
    assert!(err.to_string().contains("nope.md"), "err was: {err}");
}

#[test]
fn render_subagents_errors_on_unterminated_frontmatter() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/bad.md",
            "---\nname: x\ndescription: d\nno close\n",
        );
    });
    c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![PathBuf::from("agents/bad.md")];
    let h = c.agents().next().unwrap();
    assert!(render_subagents(&c, h).is_err());
}

#[test]
fn env_emits_claude_agents_json_for_claude_code() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(env.contains("CLAUDE_AGENTS_JSON=/teamctl/state/claude/hello-mgr.agents.json"));
}

#[test]
fn write_subagents_json_writes_then_clears_stale() {
    let (_d, mut c) = rooted(|root| {
        write_file(
            root,
            "agents/x.md",
            "---\nname: x\ndescription: d\n---\nbody\n",
        );
    });
    let dest = subagents_json_path(&c.root, "hello", "mgr");

    // Declared → file materialized.
    c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![PathBuf::from("agents/x.md")];
    let h = c.agents().next().unwrap();
    write_subagents_json(&c, h).unwrap();
    assert!(dest.exists(), "agents json should be written");

    // Dropped → stale file removed so old sub-agents don't linger.
    c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![];
    let h = c.agents().next().unwrap();
    write_subagents_json(&c, h).unwrap();
    assert!(!dest.exists(), "stale agents json should be removed");
}

#[test]
fn write_agent_skills_materializes_symlinks() {
    let (_d, mut c) = rooted(|root| {
        write_file(root, "skills/pr-review/SKILL.md", "# PR review skill\n");
    });
    c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![PathBuf::from("skills/pr-review")];
    let h = c.agents().next().unwrap();
    write_agent_skills(&c, h).unwrap();

    let link = agent_scope_dir(&c.root, "hello", "mgr").join(".claude/skills/pr-review");
    let meta = std::fs::symlink_metadata(&link).expect("link should exist");
    assert!(meta.file_type().is_symlink(), "entry must be a symlink");
    // Resolves to the source skill dir (so CC finds its SKILL.md).
    assert_eq!(
        std::fs::canonicalize(&link).unwrap(),
        std::fs::canonicalize(c.root.join("skills/pr-review")).unwrap()
    );
}

#[test]
fn write_agent_skills_clear_stale_preserves_source() {
    // SAFETY: dropping a skill must unlink only the symlink — never
    // recurse into and delete the real skill directory it pointed at.
    let (_d, mut c) = rooted(|root| {
        write_file(root, "skills/foo/SKILL.md", "# foo\n");
    });
    let source = c.root.join("skills/foo");
    let source_md = source.join("SKILL.md");

    // Declare → materialize the link.
    c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![PathBuf::from("skills/foo")];
    let h = c.agents().next().unwrap();
    write_agent_skills(&c, h).unwrap();
    let scope = agent_scope_dir(&c.root, "hello", "mgr");
    assert!(scope.join(".claude/skills/foo").exists());

    // Drop → scope cleared, but the real skill dir + SKILL.md survive.
    c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![];
    let h = c.agents().next().unwrap();
    write_agent_skills(&c, h).unwrap();
    assert!(!scope.exists(), "stale scope dir should be removed");
    assert!(source.is_dir(), "source skill dir must survive the clear");
    assert!(
        source_md.is_file(),
        "source SKILL.md must survive the clear"
    );
}

#[test]
fn write_agent_skills_isolates_per_agent() {
    // Two agents declaring different skills must each get only their
    // own — the core per-agent-scope guarantee.
    let (_d, mut c) = rooted(|root| {
        write_file(root, "skills/a/SKILL.md", "# a\n");
        write_file(root, "skills/b/SKILL.md", "# b\n");
    });
    let worker = c.projects[0].managers["mgr"].clone();
    c.projects[0].workers.insert("dev".into(), worker);
    c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![PathBuf::from("skills/a")];
    c.projects[0].workers.get_mut("dev").unwrap().skills = vec![PathBuf::from("skills/b")];

    for h in c.agents() {
        write_agent_skills(&c, h).unwrap();
    }
    let mgr_skills = agent_scope_dir(&c.root, "hello", "mgr").join(".claude/skills");
    let dev_skills = agent_scope_dir(&c.root, "hello", "dev").join(".claude/skills");
    assert!(mgr_skills.join("a").exists() && !mgr_skills.join("b").exists());
    assert!(dev_skills.join("b").exists() && !dev_skills.join("a").exists());
}

#[test]
fn write_agent_skills_ignored_on_non_claude_runtime() {
    let (_d, mut c) = rooted(|root| {
        write_file(root, "skills/x/SKILL.md", "# x\n");
    });
    {
        let a = c.projects[0].managers.get_mut("mgr").unwrap();
        a.runtime = "codex".into();
        a.skills = vec![PathBuf::from("skills/x")];
    }
    let h = c.agents().next().unwrap();
    // claude-only v1: codex ignores declared skills (warns) and no
    // scope dir is created.
    write_agent_skills(&c, h).unwrap();
    assert!(!agent_scope_dir(&c.root, "hello", "mgr").exists());
}

#[test]
fn env_emits_claude_agent_scope_for_claude_code() {
    let c = fixture();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(env.contains("CLAUDE_AGENT_SCOPE=/teamctl/state/agent-scope/hello-mgr"));
}

#[test]
fn env_omits_claude_agent_scope_for_non_claude_runtimes() {
    let mut c = fixture();
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().next().unwrap();
    let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
    assert!(
        !env.contains("CLAUDE_AGENT_SCOPE="),
        "non-claude runtime must not get the agent scope: {env}"
    );
}

#[cfg(unix)]
#[test]
fn custom_descriptor_launches_binary_with_literal_argv_and_effective_env() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("fake-runtime.sh");
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s\\n' \"$MODEL\" \"$LABEL\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(root.path().join("runtimes")).unwrap();
    let runtime = crate::runtimes::Runtime {
        binary: binary.display().to_string(),
        args: vec![
            "arg with spaces".into(),
            "quote'and*glob".into(),
            "$(false)".into(),
        ],
        supports_mcp: false,
        session_resume: None,
        default_model: Some("default-model".into()),
        env: BTreeMap::from([("LABEL".into(), "descriptor-value".into())]),
        rate_limit_patterns: vec![],
    };
    std::fs::write(
        root.path().join("runtimes/custom.yaml"),
        serde_yaml::to_string(&runtime).unwrap(),
    )
    .unwrap();
    let mut compose = fixture();
    compose.root = root.path().to_path_buf();
    let agent = compose.projects[0].managers.get_mut("mgr").unwrap();
    agent.runtime = "custom".into();
    agent.model = None;
    let handle = compose.agents().next().unwrap();
    let (env, _) = render_agent(&compose, handle, "team-mcp");
    write_runtime_launcher(&compose, handle).unwrap();
    let mut command = std::process::Command::new(runtime_launcher_path(
        &compose.root,
        handle.project,
        handle.agent,
    ));
    for line in env.lines() {
        if let Some((key, value)) = line.split_once('=') {
            command.env(key, value);
        }
    }
    let output = command.arg("forwarded").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "default-model\ndescriptor-value\narg with spaces\nquote'and*glob\n$(false)\nforwarded\n"
    );
    compose.projects[0].managers.get_mut("mgr").unwrap().model = Some("agent-model".into());
    assert!(
        render_agent(&compose, compose.agents().next().unwrap(), "team-mcp")
            .0
            .contains("MODEL=agent-model\n")
    );
}

#[test]
fn remote_mcp_auth_references_map_to_each_provider_without_resolving_secrets() {
    let mut c = fixture();
    let remote:McpServer=serde_yaml::from_str("url: https://mcp.example.com/mcp\nbearer_token_env_var: MCP_ACCESS_TOKEN\nenv_http_headers: {X-Workspace: MCP_WORKSPACE}\n").unwrap();
    c.projects[0]
        .managers
        .get_mut("mgr")
        .unwrap()
        .mcps
        .insert("remote".into(), remote);
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let (_, claude) = render_agent(&c, h, "team-mcp");
    let claude: serde_json::Value = serde_json::from_str(&claude).unwrap();
    assert_eq!(claude["mcpServers"]["remote"]["type"], "http");
    assert_eq!(
        claude["mcpServers"]["remote"]["headers"]["Authorization"],
        "Bearer ${MCP_ACCESS_TOKEN}"
    );
    assert_eq!(
        claude["mcpServers"]["remote"]["headers"]["X-Workspace"],
        "${MCP_WORKSPACE}"
    );
    assert!(claude["mcpServers"]["remote"].get("command").is_none());
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let codex = render_codex_config(&c, h, "team-mcp").unwrap();
    assert!(codex.contains("[mcp_servers.remote]\nurl = \"https://mcp.example.com/mcp\"\nbearer_token_env_var = \"MCP_ACCESS_TOKEN\"\nenv_http_headers = { X-Workspace = \"MCP_WORKSPACE\" }"));
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "opencode".into();
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let opencode: serde_json::Value =
        serde_json::from_str(&render_opencode_config(&c, h, "team-mcp").unwrap()).unwrap();
    assert_eq!(opencode["mcp"]["remote"]["type"], "remote");
    assert_eq!(
        opencode["mcp"]["remote"]["headers"]["Authorization"],
        "Bearer {env:MCP_ACCESS_TOKEN}"
    );
    assert_eq!(
        opencode["mcp"]["remote"]["headers"]["X-Workspace"],
        "{env:MCP_WORKSPACE}"
    );
    assert_eq!(opencode["mcp"]["remote"]["oauth"], false);
}

#[test]
fn stdio_mcp_forwarded_env_names_map_to_all_provider_formats() {
    let mut c = fixture();
    let server: McpServer = serde_yaml::from_str(
        "command: example-mcp\nenv_vars: [MCP_TOKEN]\nenv: {MODE: readonly}\n",
    )
    .unwrap();
    c.projects[0]
        .managers
        .get_mut("mgr")
        .unwrap()
        .mcps
        .insert("local".into(), server);
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let (_, claude) = render_agent(&c, h, "team-mcp");
    let claude: serde_json::Value = serde_json::from_str(&claude).unwrap();
    assert_eq!(
        claude["mcpServers"]["local"]["env"]["MCP_TOKEN"],
        "${MCP_TOKEN}"
    );
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let codex = render_codex_config(&c, h, "team-mcp").unwrap();
    assert!(codex.contains("env_vars = [\"MCP_TOKEN\"]\n\n[mcp_servers.local.env]"));
    c.projects[0].managers.get_mut("mgr").unwrap().runtime = "opencode".into();
    let h = c.agents().find(|a| a.agent == "mgr").unwrap();
    let opencode: serde_json::Value =
        serde_json::from_str(&render_opencode_config(&c, h, "team-mcp").unwrap()).unwrap();
    assert_eq!(
        opencode["mcp"]["local"]["environment"]["MCP_TOKEN"],
        "{env:MCP_TOKEN}"
    );
}
