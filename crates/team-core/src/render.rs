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
/// The current payload is a single `PreToolUse` deny hook covering the
/// synchronous-prompt tools that today strand a headless pane:
/// `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`. The `systemMessage`
/// tells the model *why* the deny fired and points it at the `team` MCP
/// tools as the headless-safe alternative — without that, the model just
/// sees the call vanish and may retry. Matcher is a regex; extend it
/// (rather than the hook count) when claude-code gains new synchronous-
/// prompt tools.
pub fn render_claude_settings(compose: &Compose, h: AgentHandle<'_>) -> Option<String> {
    let _ = compose;
    if h.spec.runtime != "claude-code" {
        return None;
    }
    // PreToolUse deny hook. Picked over `--disallowed-tools` so the
    // model sees the deny + systemMessage (tighter learning loop) rather
    // than the tool silently vanishing from its catalog.
    let v = serde_json::json!({
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
    Some(serde_json::to_string_pretty(&v).expect("json"))
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
    let v = serde_json::json!({
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
                model: Some("claude-opus-4-7".into()),
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
            },
        );
        Compose {
            root: PathBuf::from("/teamctl"),
            global: Global {
                version: 2,
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
}
