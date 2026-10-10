use super::*;

/// Team-mcp tool names pre-approved in the rendered codex config. Codex
/// prompts per MCP tool on first call — even under `-a never` — and
/// persists an "Always allow" answer INTO `config.toml`, which teamctl
/// rewrites on every `up`/`reload`. So the render must seed the approvals
/// or an unattended codex pane strands on its very first `inbox_peek`
/// (observed live on codex-cli 0.144.3). Keep in sync with team-mcp's
/// `schema()` tool list; a missing entry means one interactive prompt on
/// that tool's first use.
pub(super) const CODEX_PREAPPROVED_TEAM_TOOLS: &[&str] = &[
    "broadcast",
    "compact_self",
    "dm",
    "inbox_ack",
    "inbox_peek",
    "inbox_read",
    "inbox_watch",
    "list_team",
    "org_chart",
    "react_to_user",
    "read_attachment",
    "reply_to_user",
    "request_approval",
    "show_typing",
    "whoami",
];

/// Per-agent Codex `config.toml` for `runtime: codex` agents. Returns
/// `None` for every other runtime. Codex has no `--mcp-config` flag —
/// MCP servers are read from `[mcp_servers.<name>]` tables in
/// `$CODEX_HOME/config.toml` — so this file is the codex-shaped mirror of
/// [`render_mcp`]'s JSON: the unconditional `team` bus plus declared
/// `mcps:` under the same `supports_mcp` gate (render_mcp already warns
/// when the gate drops them, so this stays quiet). TOML is hand-rendered
/// with proper string escaping — team-core carries no toml crate and one
/// table shape doesn't earn the dependency.
///
/// Two extra seeded tables, both in the exact shape codex 0.144.3 itself
/// persists when a human answers its dialogs (verified live):
/// `[projects."<cwd>"] trust_level = "trusted"` skips the boot-time
/// trust dialog — whose wording has shifted across codex releases, so
/// pre-seeding beats pane-text matching (the wrapper's auto-confirm
/// patterns stay as a backstop) — and per-tool
/// `[mcp_servers.team.tools.<name>] approval_mode = "approve"` entries
/// cover the team bus. Declared `mcps:` tools are NOT pre-approved (their
/// tool lists are unknown at render time): they prompt once per tool, and
/// an attached operator's "Always allow" survives only until the next
/// render — documented limitation, follow-up ticket.
pub fn render_codex_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> Option<String> {
    if h.spec.runtime != "codex" {
        return None;
    }
    let mut s = String::from(
        "# teamctl-managed: rewritten on every `teamctl up` / `reload`.\n\
         # Customize MCP servers through the compose file's `mcps:` field.\n",
    );
    push_mcp_server_table(
        &mut s,
        "team",
        team_mcp_bin,
        &team_server_args(compose, h),
        &Default::default(),
    );
    for tool in CODEX_PREAPPROVED_TEAM_TOOLS {
        s.push_str(&format!(
            "\n[mcp_servers.team.tools.{tool}]\napproval_mode = \"approve\"\n"
        ));
    }
    if !h.spec.mcps.is_empty() && runtime_supports_mcp(compose, h) {
        for (name, server) in &h.spec.mcps {
            if name == "team" {
                continue; // non-clobberable bus; validate rejects this too
            }
            push_mcp_server_table(&mut s, name, &server.command, &server.args, &server.env);
        }
    }
    // Trust the project cwd so the boot dialog never renders. Codex keys
    // the table on the canonical absolute path of the directory it starts
    // in; fall back to the plain join when canonicalize fails (cwd not
    // yet created — the dialog would then appear and the wrapper's
    // auto-confirm backstop handles it).
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .expect("agent belongs to a loaded project");
    let cwd = compose.root.join(&project.project.cwd);
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    s.push_str(&format!(
        "\n[projects.{}]\ntrust_level = \"trusted\"\n",
        toml_str(&cwd.display().to_string())
    ));
    // Allow network from inside the workspace-write sandbox (the wrapper's
    // headless default `-s workspace-write`). Without it codex can't reach
    // github.com etc., so agents fall back to ChatGPT connector apps
    // (`codex_apps.github.*`) for git work — and those hit a per-tool
    // approval prompt teamctl can't pre-seed (the connector id is
    // account-specific) and would strand an unattended pane. With network
    // on, agents use plain `git`/`gh`/`curl`, the same tooling claude-code
    // agents already have. The filesystem stays sandboxed to the workspace
    // + temp dirs, so this is still tighter than claude-code (which isn't
    // network-sandboxed at all). Inert under `attended`/`--yolo` sandboxes.
    s.push_str("\n[sandbox_workspace_write]\nnetwork_access = true\n");
    Some(s)
}

/// Write (or clear) the per-agent Codex `config.toml`. Mirrors
/// [`write_subagents_json`]: the scoped + full render paths both call it
/// so a `mcps:` edit flows into the agent at the next render. For
/// non-codex agents only the managed `config.toml` is removed — never the
/// home dir itself, which may hold codex session history worth keeping.
pub fn write_codex_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> io::Result<()> {
    let dest = codex_home_dir(&compose.root, h.project, h.agent).join("config.toml");
    match render_codex_config(compose, h, team_mcp_bin) {
        Some(toml) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::persistence::atomic_write(&dest, toml.as_bytes())
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

/// Per-agent OpenCode `opencode.json` for `runtime: opencode` agents.
/// Returns `None` for every other runtime. OpenCode has no
/// `--mcp-config` flag — servers are read from the `mcp` object of the
/// Append one `[mcp_servers.<name>]` table (plus its `.env` sub-table
/// when non-empty) in the shape codex reads from `config.toml`.
fn push_mcp_server_table(
    buf: &mut String,
    name: &str,
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) {
    buf.push_str(&format!("\n[mcp_servers.{}]\n", toml_key(name)));
    buf.push_str(&format!("command = {}\n", toml_str(command)));
    let args: Vec<String> = args.iter().map(|a| toml_str(a)).collect();
    buf.push_str(&format!("args = [{}]\n", args.join(", ")));
    if !env.is_empty() {
        buf.push_str(&format!("\n[mcp_servers.{}.env]\n", toml_key(name)));
        for (k, v) in env {
            buf.push_str(&format!("{} = {}\n", toml_key(k), toml_str(v)));
        }
    }
}

/// Render a string as a TOML basic string (double-quoted). Backslash,
/// quote, and control characters are the only escapes basic strings
/// require; everything else passes through verbatim (values are not
/// interpolated, matching render's no-`${VAR}`-expansion rule).
pub(super) fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render a TOML key: bare when it fits TOML's bare-key charset, quoted
/// otherwise (server names and env keys come from operator YAML and
/// aren't constrained to bare-safe characters).
pub(super) fn toml_key(k: &str) -> String {
    let bare = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        k.to_string()
    } else {
        toml_str(k)
    }
}
