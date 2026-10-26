use super::*;

/// Args for the built-in team MCP server, shared by every runtime config.
pub(super) fn team_server_args(compose: &Compose, h: AgentHandle<'_>) -> Vec<String> {
    let mailbox = compose.root.join(&compose.global.broker.path);
    vec![
        "--agent-id".into(),
        format!("{}:{}", h.project, h.agent),
        "--mailbox".into(),
        mailbox.display().to_string(),
        // T-109: compact_self resolves the caller's tmux pane
        // as `<prefix><project>-<agent>`. Pass the configured
        // prefix explicitly so teams overriding the default
        // (`a-`, `oss-`, …) route the slash command to the
        // right session. team-bot gets the same arg threaded
        // from `teamctl bot up`; this keeps the two MCP-side
        // and bot-side resolvers in sync.
        "--tmux-prefix".into(),
        compose.global.supervisor.tmux_prefix.clone(),
        // T-32b: compose root used by `read_attachment`
        // for `attachments:` policy + tempfile staging.
        // Always passed so the per-agent team-mcp can
        // serve attachment reads; the staging dir is
        // computed under this root.
        "--compose-root".into(),
        compose.root.display().to_string(),
    ]
}

/// Whether declared `mcps:` render for this agent's runtime. Fail open
/// when the descriptor is missing: an unknown runtime is flagged at
/// validate, and a load failure shouldn't silently drop declared servers.
pub(super) fn runtime_supports_mcp(compose: &Compose, h: AgentHandle<'_>) -> bool {
    let runtimes = crate::runtimes::load_all(&compose.root).unwrap_or_default();
    runtimes
        .get(h.spec.runtime.as_str())
        .map(|r| r.supports_mcp)
        .unwrap_or(true)
}

pub(super) fn render_mcp(compose: &Compose, h: AgentHandle<'_>, team_mcp_bin: &str) -> String {
    let team_mcp_bin = container_mcp_bin(h, team_mcp_bin);
    let mut v = serde_json::json!({
        "mcpServers": {
            "team": {
                "command": team_mcp_bin,
                "args": team_server_args(compose, h),
                "env": {}
            }
        }
    });

    // #383 Phase 4: merge per-agent declared MCP servers alongside the
    // built-in `team` server. Unlike hooks (claude-only), MCP is the
    // runtime-agnostic bus, so declared servers render for every runtime
    // whose descriptor sets `supports_mcp`. The `team` server is the
    // mailbox transport: it stays unconditional and non-clobberable — a
    // declared server named `team` is skipped here (and rejected at
    // validate) so it can never shadow the bus. env values pass through
    // verbatim — render never expands `${VAR}` placeholders, since that
    // would write resolved secrets to disk. Claude Code expands them at
    // launch; codex does NOT interpolate config.toml values, so validate
    // warns on codex agents (McpEnvInterpolationUnsupported).
    if !h.spec.mcps.is_empty() {
        if runtime_supports_mcp(compose, h) {
            let servers = v["mcpServers"]
                .as_object_mut()
                .expect("mcpServers is a json object");
            for (name, server) in &h.spec.mcps {
                if name == "team" {
                    continue; // non-clobberable bus; validate rejects this too
                }
                servers.insert(name.clone(), claude_mcp_server(server));
            }
        } else {
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} MCP server(s) but runtime `{}` does not set `supports_mcp`; ignoring",
                h.project,
                h.agent,
                h.spec.mcps.len(),
                h.spec.runtime
            );
        }
    }

    serde_json::to_string_pretty(&v).expect("json")
}

pub(super) fn container_mcp_bin<'a>(h: AgentHandle<'_>, host: &'a str) -> &'a str {
    if h.spec
        .execution
        .as_ref()
        .is_some_and(|e| e.sandbox.r#type == crate::compose::SandboxType::Docker)
    {
        "/usr/local/bin/team-mcp"
    } else {
        host
    }
}

fn claude_mcp_server(server: &crate::compose::McpServer) -> serde_json::Value {
    if let Some(url) = &server.url {
        let mut headers = serde_json::Map::new();
        for (name, variable) in &server.env_http_headers {
            headers.insert(name.clone(), serde_json::json!(format!("${{{variable}}}")));
        }
        if let Some(variable) = &server.bearer_token_env_var {
            headers.insert(
                "Authorization".into(),
                serde_json::json!(format!("Bearer ${{{variable}}}")),
            );
        }
        serde_json::json!({"type":"http","url":url,"headers":headers})
    } else {
        let mut env = server.env.clone();
        for variable in &server.env_vars {
            env.insert(variable.clone(), format!("${{{variable}}}"));
        }
        serde_json::json!({"command":server.command,"args":server.args,"env":env})
    }
}
