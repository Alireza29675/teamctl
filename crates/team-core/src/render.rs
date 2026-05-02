//! Render a loaded compose into on-disk artifacts.
//!
//! Outputs under `<root>/state/`:
//! - `envs/<project>-<agent>.env`      — env vars for the agent wrapper.
//! - `mcp/<project>-<agent>.json`      — MCP stdio config for the runtime.
//!
//! `systemd` / `launchd` unit rendering lives behind a feature flag when
//! those back-ends are enabled via `supervisor.type`.

use std::path::{Path, PathBuf};

use crate::compose::{AgentHandle, Compose};

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

fn render_env(compose: &Compose, h: AgentHandle<'_>) -> String {
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .expect("agent belongs to a loaded project");
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mcp = mcp_path(&compose.root, h.project, h.agent);
    let prompt = h
        .spec
        .role_prompt
        .as_ref()
        .map(|p| compose.root.join(p))
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
    s.push_str(&format!(
        "TMUX_SESSION={}{}-{}\n",
        compose.global.supervisor.tmux_prefix, h.project, h.agent
    ));
    s
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
                role_prompt: Some(PathBuf::from("roles/mgr.md")),
                permission_mode: Some("auto".into()),
                autonomy: "low_risk_only".into(),
                can_dm: vec![],
                can_broadcast: vec![],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
                interfaces: None,
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
}
