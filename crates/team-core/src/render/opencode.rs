use super::*;

/// config file `OPENCODE_CONFIG` points at — so this is the
/// opencode-shaped mirror of [`render_mcp`]'s JSON: the unconditional
/// `team` bus plus declared `mcps:` under the same `supports_mcp` gate
/// (render_mcp already warns when the gate drops them, so this stays
/// quiet). The file also carries:
/// - `instructions`: the absolute role-prompt path (opencode has no
///   system-prompt flag; absolute-path entries verified to load),
/// - `autoupdate: false`: the TUI otherwise upgrades the shared binary
///   in place mid-fleet (the wrapper also exports
///   `OPENCODE_DISABLE_AUTOUPDATE=1` — defense in depth, upstream has
///   open bugs about this key being ignored on some paths),
/// - `share: "disabled"`: agents must never leak conversations to
///   opencode's public share links.
///
/// HAZARD (verified on 1.17.13): opencode drops schema violations
/// SILENTLY — renaming `command` made the server vanish with no error —
/// so the tests pin the exact `type`/`command`/`environment`/`enabled`
/// key names.
pub fn render_opencode_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> Option<String> {
    if h.spec.runtime != "opencode" {
        return None;
    }
    let team_mcp_bin = super::mcp::container_mcp_bin(h, team_mcp_bin);
    let mut mcp = serde_json::Map::new();
    mcp.insert(
        "team".into(),
        opencode_mcp_server(
            team_mcp_bin,
            &team_server_args(compose, h),
            &Default::default(),
        ),
    );
    if !h.spec.mcps.is_empty() && runtime_supports_mcp(compose, h) {
        for (name, server) in &h.spec.mcps {
            if name == "team" {
                continue; // non-clobberable bus; validate rejects this too
            }
            mcp.insert(
                name.clone(),
                if let Some(url) = &server.url {
                    let mut headers = serde_json::Map::new();
                    for (name, variable) in &server.env_http_headers {
                        headers.insert(
                            name.clone(),
                            serde_json::json!(format!("{{env:{variable}}}")),
                        );
                    }
                    if let Some(variable) = &server.bearer_token_env_var {
                        headers.insert(
                            "Authorization".into(),
                            serde_json::json!(format!("Bearer {{env:{variable}}}")),
                        );
                    }
                    let mut remote = serde_json::json!({"type":"remote","url":url,"enabled":true});
                    if !headers.is_empty() {
                        remote["headers"] = serde_json::json!(headers);
                        remote["oauth"] = serde_json::json!(false);
                    }
                    remote
                } else {
                    let mut env = server.env.clone();
                    for variable in &server.env_vars {
                        env.insert(variable.clone(), format!("{{env:{variable}}}"));
                    }
                    opencode_mcp_server(&server.command, &server.args, &env)
                },
            );
        }
    }
    let mut v = serde_json::json!({
        "mcp": mcp,
        "autoupdate": false,
        "share": "disabled",
    });
    if let Some(prompt) = system_prompt_path(compose, h) {
        v["instructions"] = serde_json::json!([prompt.display().to_string()]);
    }
    Some(serde_json::to_string_pretty(&v).expect("json"))
}

/// One entry of opencode's `mcp` config object. Key names are load-
/// bearing: opencode silently drops entries that violate its schema,
/// so `type`/`command`/`environment`/`enabled` must be spelled exactly.
/// `command` is a single argv array (binary first), unlike the
/// command/args split of [`render_mcp`]'s JSON; `environment` is
/// emitted only when non-empty, values verbatim (no `${VAR}`
/// expansion in render).
fn opencode_mcp_server(
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) -> serde_json::Value {
    let mut argv = vec![command.to_string()];
    argv.extend(args.iter().cloned());
    let mut server = serde_json::json!({
        "type": "local",
        "command": argv,
        "enabled": true,
    });
    if !env.is_empty() {
        server["environment"] = serde_json::to_value(env).expect("json");
    }
    server
}

/// Write (or clear) the per-agent OpenCode `opencode.json`. Mirrors
/// [`write_codex_config`]: the scoped + full render paths both call it
/// so a `mcps:` edit flows into the agent at the next render. For
/// non-opencode agents only the managed `opencode.json` is removed —
/// never the home dir itself, which holds the agent's session db.
pub fn write_opencode_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> io::Result<()> {
    let dest = opencode_home_dir(&compose.root, h.project, h.agent).join("opencode.json");
    match render_opencode_config(compose, h, team_mcp_bin) {
        Some(json) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::persistence::atomic_write(&dest, json.as_bytes())
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}
