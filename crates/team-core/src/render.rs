//! Render a loaded compose into on-disk artifacts.
//!
//! Outputs under `<root>/state/`:
//! - `envs/<project>-<agent>.env`      — env vars for the agent wrapper.
//! - `mcp/<project>-<agent>.json`      — MCP stdio config for the runtime.
//! - `claude/<project>-<agent>.json`   — wrapper-managed Claude Code
//!   settings (currently a `PreToolUse` deny hook for synchronous-prompt
//!   tools that strand a headless pane). Claude-code agents only.
//! - `role_prompts/<project>-<agent>.md` (multi-file role_prompt only) —
//!   the ordered concatenation of every source file declared in the
//!   role's `role_prompt: [...]` list. Re-materialized on every render
//!   so any source-file edit lands in the agent's prompt at next boot.
//!
//! `systemd` / `launchd` unit rendering lives behind a feature flag when
//! those back-ends are enabled via `supervisor.type`.

use std::io;
use std::path::{Path, PathBuf};

use crate::compose::{AgentHandle, Compose, RolePrompt};

/// Separator written between concatenated role-prompt files. Em-dash
/// framed by blank lines reads cleanly when an operator inspects the
/// materialized file under `state/role_prompts/`.
const ROLE_PROMPT_SEPARATOR: &str = "\n\n—\n\n";

/// Absolute path to the rendered env file for a given agent.
pub fn env_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/envs")
        .join(format!("{project}-{agent}.env"))
}

/// Absolute path to the rendered MCP config for a given agent.
pub fn mcp_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/mcp")
        .join(format!("{project}-{agent}.json"))
}

/// Absolute path to the wrapper-managed Claude Code settings file. The
/// file carries the default `PreToolUse` deny hook for synchronous-prompt
/// tools (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`) so a
/// headless agent doesn't strand on a picker no one will answer. The
/// wrapper applies it via `claude --settings <path>` for every
/// claude-code agent except those in `permission_mode: attended`.
pub fn claude_settings_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/claude")
        .join(format!("{project}-{agent}.json"))
}

/// Absolute path to the rendered Claude Code `--agents` JSON for one agent
/// (#383 Phase 3a). Lives beside the settings file under `state/claude/`
/// and is written only when the agent declares `subagents:`; the wrapper
/// passes it via `--agents "$(cat <path>)"` when the file exists.
pub fn subagents_json_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/claude")
        .join(format!("{project}-{agent}.agents.json"))
}

/// Absolute path to the materialized concatenation of a multi-file
/// `role_prompt` list. Only ever written for the list form — single-file
/// `role_prompt` keeps pointing at its source path directly.
pub fn role_prompt_concat_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/role_prompts")
        .join(format!("{project}-{agent}.md"))
}

/// Rendered env + MCP content for a single agent.
pub fn render_agent(
    compose: &Compose,
    handle: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> (String, String) {
    let env = render_env(compose, handle);
    let mcp = render_mcp(compose, handle, team_mcp_bin);
    (env, mcp)
}

/// Wrapper-managed Claude Code settings JSON for a single agent. Returns
/// `Some(json)` for `claude-code` runtime regardless of `permission_mode`
/// — the wrapper decides whether to apply it. Returns `None` for runtimes
/// that don't read Claude settings (codex, gemini, …).
///
/// The base payload is a single `PreToolUse` deny hook covering the
/// synchronous-prompt tools that today strand a headless pane:
/// `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`. The `systemMessage`
/// tells the model *why* the deny fired and points it at the `team` MCP
/// tools as the headless-safe alternative — without that, the model just
/// sees the call vanish and may retry. Matcher is a regex; extend it
/// (rather than the hook count) when claude-code gains new synchronous-
/// prompt tools.
///
/// #383 Phase 2: per-agent hooks declared in compose (`Agent.hooks`) are
/// merged on top of that base. Each declaration is appended as its own
/// entry under its event, so the built-in deny hook keeps its slot and a
/// user hook can extend behavior but not clobber the interactive-prompt
/// deny. Hook commands are compose-root-relative and rendered absolute.
pub fn render_claude_settings(compose: &Compose, h: AgentHandle<'_>) -> Option<String> {
    if h.spec.runtime != "claude-code" {
        // Hooks are a Claude-Code concept. On other runtimes the whole
        // settings file is skipped; surface a warning so a declared-but-
        // ignored hook isn't silently dropped (claude-only v1).
        if !h.spec.hooks.is_empty() {
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} hook(s) but runtime `{}` does not support hooks (claude-code only); ignoring",
                h.project,
                h.agent,
                h.spec.hooks.len(),
                h.spec.runtime
            );
        }
        return None;
    }
    // PreToolUse deny hook. Picked over `--disallowed-tools` so the
    // model sees the deny + systemMessage (tighter learning loop) rather
    // than the tool silently vanishing from its catalog. Emitted first
    // and never removed; declared hooks (below) are appended after it.
    let mut v = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "AskUserQuestion|EnterPlanMode|ExitPlanMode",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\"},\"systemMessage\":\"Interactive prompts are disabled for teamctl agents. Use the `team` MCP tools to ask people or check in.\"}'"
                        }
                    ]
                }
            ]
        }
    });

    // #383 Phase 2: merge per-agent declared hooks on top. Each
    // declaration becomes its own entry appended to its event's array, so
    // the built-in deny hook above always keeps its slot. Commands are
    // compose-root-relative (like `role_prompt`), rendered as absolute
    // paths.
    let hooks_obj = v["hooks"].as_object_mut().expect("hooks is a json object");
    for hook in &h.spec.hooks {
        let command = compose.root.join(&hook.command);
        let mut entry = serde_json::json!({
            "hooks": [
                {
                    "type": "command",
                    "command": command.display().to_string()
                }
            ]
        });
        if let Some(matcher) = &hook.matcher {
            entry["matcher"] = serde_json::Value::String(matcher.clone());
        }
        hooks_obj
            .entry(hook.event.clone())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("hook event maps to a json array")
            .push(entry);
    }

    Some(serde_json::to_string_pretty(&v).expect("json"))
}

/// #383 Phase 3a: build Claude Code's `--agents` inline JSON for one agent
/// from its declared `subagents:` list. Each list entry is a
/// compose-root-relative markdown file with standard sub-agent frontmatter
/// (`name`, `description`, optional `tools`, `model`) and a body that
/// becomes the sub-agent's system `prompt`. The result is the
/// `{ "<name>": { description, prompt, [tools], [model] } }` object the
/// `--agents` flag consumes — the only cwd-stationary way to scope
/// sub-agents per agent (no arbitrary-path flag exists; see the Phase-1
/// spike). Returns `Ok(None)` when none are declared (→ no `--agents`
/// flag) or the runtime isn't claude-code (logs an "unsupported" warning,
/// claude-only v1); `Err` if a source is unreadable or its frontmatter is
/// invalid, so a typo fails the apply loudly rather than dropping a
/// sub-agent silently.
pub fn render_subagents(compose: &Compose, h: AgentHandle<'_>) -> io::Result<Option<String>> {
    if h.spec.subagents.is_empty() {
        return Ok(None);
    }
    if h.spec.runtime != "claude-code" {
        tracing::warn!(
            target: "team-core::render",
            "agent `{}:{}` declares {} sub-agent(s) but runtime `{}` does not support sub-agents (claude-code only); ignoring",
            h.project,
            h.agent,
            h.spec.subagents.len(),
            h.spec.runtime
        );
        return Ok(None);
    }

    let mut map = serde_json::Map::new();
    for rel in &h.spec.subagents {
        let abs = compose.root.join(rel);
        let raw = std::fs::read_to_string(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read sub-agent source {}: {e}", abs.display()),
            )
        })?;
        let (fm, body) = parse_subagent(&raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse sub-agent {}: {e}", abs.display()),
            )
        })?;
        // Name from frontmatter, else the file stem (so `agents/foo.md`
        // without an explicit `name:` registers as sub-agent `foo`).
        let name = fm.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            rel.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let mut entry = serde_json::json!({
            "description": fm.description,
            "prompt": body,
        });
        if let Some(tools) = fm.tools {
            let list = tools.into_list();
            if !list.is_empty() {
                entry["tools"] = serde_json::json!(list);
            }
        }
        if let Some(model) = fm.model.filter(|m| !m.trim().is_empty()) {
            entry["model"] = serde_json::Value::String(model);
        }
        map.insert(name, entry);
    }
    Ok(Some(
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).expect("json"),
    ))
}

/// Write (or clear) the per-agent `--agents` JSON file. Mirrors
/// [`write_role_prompt_concat`]: the scoped + full render paths both call
/// it so a `subagents:` edit flows into the agent at the next render. When
/// the agent declares no sub-agents (or isn't claude-code) the file is
/// removed if present, so a stale `--agents` set never lingers across a
/// reload that dropped them.
pub fn write_subagents_json(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let dest = subagents_json_path(&compose.root, h.project, h.agent);
    match render_subagents(compose, h)? {
        Some(json) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, json)
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

/// Parsed frontmatter of a sub-agent markdown file. Mirrors the fields
/// Claude Code's own `.claude/agents/*.md` use; unknown keys are ignored.
#[derive(serde::Deserialize)]
struct SubagentFrontmatter {
    #[serde(default)]
    name: Option<String>,
    description: String,
    #[serde(default)]
    tools: Option<Tools>,
    #[serde(default)]
    model: Option<String>,
}

/// `tools:` accepts either Claude Code's comma-separated string form
/// (`Read, Grep`) or a YAML list (`[Read, Grep]`); both normalize to the
/// JSON array `--agents` expects.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Tools {
    List(Vec<String>),
    Csv(String),
}

impl Tools {
    fn into_list(self) -> Vec<String> {
        let raw = match self {
            Tools::List(v) => v,
            Tools::Csv(s) => s.split(',').map(str::to_string).collect(),
        };
        raw.into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Split a sub-agent markdown file into (frontmatter, body). Expects the
/// standard `---\n<yaml>\n---\n<body>` layout; the body is everything after
/// the closing delimiter, trimmed of surrounding blank lines.
fn parse_subagent(raw: &str) -> Result<(SubagentFrontmatter, String), String> {
    let after_open = raw
        .strip_prefix("---")
        .ok_or("missing opening `---` frontmatter delimiter")?;
    let (yaml, body) = after_open
        .split_once("\n---")
        .ok_or("missing closing `---` frontmatter delimiter")?;
    let fm: SubagentFrontmatter =
        serde_yaml::from_str(yaml.trim()).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;
    let body = body.trim_start_matches(['\r', '\n']).trim_end().to_string();
    Ok((fm, body))
}

fn render_env(compose: &Compose, h: AgentHandle<'_>) -> String {
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .expect("agent belongs to a loaded project");
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mcp = mcp_path(&compose.root, h.project, h.agent);
    let prompt = system_prompt_path(compose, h)
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut s = String::new();
    s.push_str(&format!("AGENT_ID={}:{}\n", h.project, h.agent));
    s.push_str(&format!("PROJECT_ID={}\n", h.project));
    s.push_str(&format!("RUNTIME={}\n", h.spec.runtime));
    if let Some(m) = &h.spec.model {
        s.push_str(&format!("MODEL={m}\n"));
    }
    if let Some(pm) = &h.spec.permission_mode {
        s.push_str(&format!("PERMISSION_MODE={pm}\n"));
    }
    // T-048: per-agent reasoning effort flows through to the runtime
    // via the wrapper. Workspace-level `.env` `EFFORT=` still wins for
    // operators not yet on the YAML form (back-compat).
    if let Some(effort) = h.spec.effort {
        s.push_str(&format!("EFFORT={}\n", effort.as_str()));
    }
    s.push_str(&format!("TEAMCTL_MAILBOX={}\n", mailbox.display()));
    s.push_str(&format!("MCP_CONFIG={}\n", mcp.display()));
    s.push_str(&format!("SYSTEM_PROMPT_PATH={prompt}\n"));
    s.push_str(&format!(
        "CLAUDE_PROJECT_DIR={}\n",
        project.project.cwd.display()
    ));
    // Absolute path to the compose root (the directory holding
    // `team-compose.yaml`). The wrapper passes this to `teamctl --root`
    // so rl-watch resolves the right tree regardless of where
    // `cd "$CLAUDE_PROJECT_DIR"` lands the shell. Without this,
    // wrapper falls back to CLAUDE_PROJECT_DIR (often a relative `..`)
    // which compounds with the post-cd cwd and points at the wrong
    // directory.
    s.push_str(&format!("TEAMCTL_ROOT={}\n", compose.root.display()));
    s.push_str(&format!(
        "TMUX_SESSION={}{}-{}\n",
        compose.global.supervisor.tmux_prefix, h.project, h.agent
    ));
    // T-118: claude-code agents resume their conversation across
    // teamctl down/up + crash recovery via a deterministic UUIDv5
    // session id. Other runtimes don't recognize `--session-id`, so
    // emit these env vars only for `claude-code` — the wrapper's
    // claude-code arm picks them up; other arms ignore them.
    if h.spec.runtime == "claude-code" {
        let session_id = crate::session::derive_session_id(h.project, h.agent);
        let session_name = crate::session::session_name(h.project, h.agent);
        s.push_str(&format!("CLAUDE_SESSION_ID={session_id}\n"));
        s.push_str(&format!("CLAUDE_SESSION_NAME={session_name}\n"));
        // T-189: path to the wrapper-managed Claude settings file
        // carrying the synchronous-prompt deny hook. Wrapper applies
        // it via `--settings` except when `permission_mode: attended`
        // (human at the keyboard wants the interactive tools back).
        let settings = claude_settings_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_SETTINGS={}\n", settings.display()));
        // #383 Phase 3a: path to the rendered `--agents` JSON carrying this
        // agent's declared sub-agents. Always emitted for claude-code; the
        // file itself is written only when `subagents:` is non-empty, so
        // the wrapper's `[ -f ]` guard decides whether `--agents` is passed.
        let subagents = subagents_json_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_AGENTS_JSON={}\n", subagents.display()));
    }
    s
}

/// Resolve the absolute path that `SYSTEM_PROMPT_PATH` will point at.
///
/// - `None` role_prompt → `None` (env line renders as blank).
/// - Single source file → `<root>/<source>` (back-compat, no concat
///   file is written — the operator's source is the prompt).
/// - List form → the materialized concat path under
///   `<root>/state/role_prompts/<project>-<agent>.md`. The file at that
///   path is produced by [`write_role_prompt_concat`]; this helper is
///   pure and only computes the destination.
pub fn system_prompt_path(compose: &Compose, h: AgentHandle<'_>) -> Option<PathBuf> {
    match h.spec.role_prompt.as_ref()? {
        RolePrompt::Single(p) => Some(compose.root.join(p)),
        RolePrompt::Multiple(_) => Some(role_prompt_concat_path(&compose.root, h.project, h.agent)),
    }
}

/// Materialize the multi-file `role_prompt` concatenation for one agent.
///
/// No-op when `role_prompt` is `None` or `Single` — there is nothing to
/// concatenate. For the list form, every source file is read in declared
/// order and joined with [`ROLE_PROMPT_SEPARATOR`]; the result overwrites
/// `<root>/state/role_prompts/<project>-<agent>.md` so subsequent edits
/// to any source file flow into the agent's prompt at the next render.
///
/// Missing source files surface as the underlying `io::Error` so the
/// caller can fail the apply rather than silently emit a partial concat.
pub fn write_role_prompt_concat(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let Some(RolePrompt::Multiple(paths)) = h.spec.role_prompt.as_ref() else {
        return Ok(());
    };

    let mut buf = String::new();
    for (idx, rel) in paths.iter().enumerate() {
        if idx > 0 {
            buf.push_str(ROLE_PROMPT_SEPARATOR);
        }
        let abs = compose.root.join(rel);
        let bytes = std::fs::read(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read role_prompt source {}: {e}", abs.display()),
            )
        })?;
        // Source files are expected to be UTF-8 markdown; lossy decode
        // keeps render diagnostics readable if a stray byte sneaks in.
        buf.push_str(&String::from_utf8_lossy(&bytes));
    }

    let dest = role_prompt_concat_path(&compose.root, h.project, h.agent);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, buf)
}

fn render_mcp(compose: &Compose, h: AgentHandle<'_>, team_mcp_bin: &str) -> String {
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mut v = serde_json::json!({
        "mcpServers": {
            "team": {
                "command": team_mcp_bin,
                "args": [
                    "--agent-id", format!("{}:{}", h.project, h.agent),
                    "--mailbox", mailbox.display().to_string(),
                    // T-109: compact_self resolves the caller's tmux pane
                    // as `<prefix><project>-<agent>`. Pass the configured
                    // prefix explicitly so teams overriding the default
                    // (`a-`, `oss-`, …) route the slash command to the
                    // right session. team-bot gets the same arg threaded
                    // from `teamctl bot up`; this keeps the two MCP-side
                    // and bot-side resolvers in sync.
                    "--tmux-prefix", compose.global.supervisor.tmux_prefix.clone(),
                    // T-32b: compose root used by `read_attachment`
                    // for `attachments:` policy + tempfile staging.
                    // Always passed so the per-agent team-mcp can
                    // serve attachment reads; the staging dir is
                    // computed under this root.
                    "--compose-root", compose.root.display().to_string(),
                ],
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
    // verbatim; the runtime performs any `${VAR}` expansion.
    if !h.spec.mcps.is_empty() {
        let runtimes = crate::runtimes::load_all(&compose.root).unwrap_or_default();
        // Fail open when the descriptor is missing: an unknown runtime is
        // flagged at validate, and a load failure shouldn't silently drop
        // declared servers.
        let supports_mcp = runtimes
            .get(h.spec.runtime.as_str())
            .map(|r| r.supports_mcp)
            .unwrap_or(true);
        if supports_mcp {
            let servers = v["mcpServers"]
                .as_object_mut()
                .expect("mcpServers is a json object");
            for (name, server) in &h.spec.mcps {
                if name == "team" {
                    continue; // non-clobberable bus; validate rejects this too
                }
                servers.insert(
                    name.clone(),
                    serde_json::to_value(server).expect("serialize McpServer"),
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn fixture() -> Compose {
        let mut managers = BTreeMap::new();
        managers.insert(
            "mgr".into(),
            Agent {
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
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
            },
        );
        Compose {
            root: PathBuf::from("/teamctl"),
            global: Global {
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
        let i = args.iter().position(|a| *a == "--tmux-prefix").expect(
            "render_mcp must emit --tmux-prefix so compact_self resolves the caller's pane",
        );
        assert_eq!(
            args[i + 1],
            "a-",
            "prefix must come from compose, not the default"
        );
    }

    /// Build a `McpServer` test value tersely.
    fn server(command: &str, args: &[&str]) -> McpServer {
        McpServer {
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
            "env values must pass through verbatim — the runtime expands ${{VAR}}"
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
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
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
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
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
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
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
    fn claude_settings_absent_for_non_claude_runtimes() {
        // codex/gemini don't read claude settings; the file would be
        // dead weight and a confusing artifact on disk.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        assert!(render_claude_settings(&c, h).is_none());
    }

    #[test]
    fn declared_hook_merges_alongside_deny_hook() {
        // #383 Phase 2: a per-agent hook is appended AFTER the built-in
        // deny hook in the same PreToolUse bucket — the deny keeps slot 0
        // and the command resolves compose-root-relative to absolute.
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
        assert_eq!(pre.len(), 2, "deny hook + declared hook expected");
        // Built-in deny hook survives in slot 0.
        assert_eq!(
            pre[0]["matcher"].as_str().unwrap(),
            "AskUserQuestion|EnterPlanMode|ExitPlanMode"
        );
        assert!(pre[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(r#""permissionDecision":"deny""#));
        // Declared hook appended after it.
        assert_eq!(pre[1]["matcher"].as_str().unwrap(), "Bash");
        assert_eq!(pre[1]["hooks"][0]["type"].as_str().unwrap(), "command");
        assert_eq!(
            pre[1]["hooks"][0]["command"].as_str().unwrap(),
            "/teamctl/hooks/guard.sh"
        );
    }

    #[test]
    fn no_declared_hooks_leaves_settings_unchanged() {
        // #383 Phase 2: empty `hooks` (the default) must render exactly
        // the built-in deny hook and nothing else.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(
            hooks.len(),
            1,
            "only the built-in PreToolUse bucket expected"
        );
        assert_eq!(
            hooks["PreToolUse"].as_array().unwrap().len(),
            1,
            "only the deny hook expected"
        );
    }

    #[test]
    fn declared_hook_without_matcher_opens_new_event_bucket() {
        // #383 Phase 2: a hook on a fresh event (no matcher) creates its
        // own bucket and omits `matcher` so Claude Code matches all tools;
        // the deny hook's PreToolUse bucket is left untouched.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().hooks = vec![HookSpec {
            event: "PostToolUse".into(),
            matcher: None,
            command: PathBuf::from("hooks/log.sh"),
        }];
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
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
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(RolePrompt::Multiple(
            vec![PathBuf::from("roles/missing.md")],
        ));
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
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/x.md")];
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
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/a.md")];
        c.projects[0].workers.get_mut("dev").unwrap().subagents =
            vec![PathBuf::from("agents/b.md")];

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
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/bad.md")];
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
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/x.md")];
        let h = c.agents().next().unwrap();
        write_subagents_json(&c, h).unwrap();
        assert!(dest.exists(), "agents json should be written");

        // Dropped → stale file removed so old sub-agents don't linger.
        c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![];
        let h = c.agents().next().unwrap();
        write_subagents_json(&c, h).unwrap();
        assert!(!dest.exists(), "stale agents json should be removed");
    }
}
