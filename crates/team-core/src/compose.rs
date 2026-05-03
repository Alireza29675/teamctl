//! YAML schema for `team-compose.yaml` and `projects/<id>.yaml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level `team-compose.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Global {
    pub version: u32,

    #[serde(default)]
    pub broker: Broker,

    #[serde(default)]
    pub supervisor: SupervisorCfg,

    #[serde(default)]
    pub budget: Budget,

    #[serde(default)]
    pub hitl: Hitl,

    #[serde(default)]
    pub rate_limits: RateLimits,

    /// Human-facing inbound channels. Telegram is one adapter; Discord,
    /// iMessage, CLI, and webhook share the same shape.
    #[serde(default)]
    pub interfaces: Vec<Interface>,

    /// Relative paths from the compose root.
    #[serde(default)]
    pub projects: Vec<ProjectRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interface {
    /// Adapter type: `telegram`, `discord`, `imessage`, `cli`, `webhook`, ...
    pub r#type: String,
    /// Free-form name; used in logs and to route approvals.
    pub name: String,
    /// Adapter-specific config (bot token, channel id, allowlist, …).
    #[serde(default)]
    pub config: serde_yaml::Value,
}

impl Interface {
    pub fn is_telegram(&self) -> bool {
        self.r#type == "telegram"
    }

    /// `<project>:<manager>` this interface routes to, when set.
    pub fn manager(&self) -> Option<String> {
        self.config_str("manager")
    }

    /// Env var name holding the bot token (e.g. `TEAMCTL_TG_PM_TOKEN`).
    pub fn bot_token_env(&self) -> Option<String> {
        self.config_str("bot_token_env")
    }

    /// Env var name holding a comma-separated allow-list of chat ids.
    pub fn authorized_chat_ids_env(&self) -> Option<String> {
        self.config_str("authorized_chat_ids_env")
    }

    fn config_str(&self, key: &str) -> Option<String> {
        match &self.config {
            serde_yaml::Value::Mapping(m) => m
                .get(serde_yaml::Value::String(key.into()))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Budget {
    #[serde(default)]
    pub daily_usd_limit: Option<f64>,
    #[serde(default)]
    pub warn_threshold_pct: Option<u32>,
    #[serde(default)]
    pub message_ttl_hours: Option<u32>,
    #[serde(default)]
    pub per_project_usd_limit: std::collections::BTreeMap<String, f64>,
}

/// Rate-limit handling policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimits {
    /// Default hook-name chain to run on a hit. Empty means `[wait]`.
    #[serde(default)]
    pub default_on_hit: Vec<String>,

    /// Named hooks. Agents reference these by name in their `on_rate_limit:`.
    #[serde(default)]
    pub hooks: Vec<RateLimitHook>,

    /// Fallback wait when the hit can't be parsed for a reset time.
    /// Default 30 minutes.
    #[serde(default = "default_fallback_wait")]
    pub fallback_wait_seconds: u64,
}

fn default_fallback_wait() -> u64 {
    30 * 60
}

/// One named action that can run on a rate-limit hit.
///
/// `action` is one of:
/// - `wait` — sleep until `resets_at` (or `fallback_wait_seconds`).
/// - `send` — write a message into the mailbox; `to` and `template` required.
/// - `webhook` — POST/GET to `url` (or `url_env`); the rate-limit row
///   serializes as JSON in the body.
/// - `run` — exec `command` with placeholders substituted.
///
/// Placeholders in `template` and `command` arguments:
/// `{agent}`, `{runtime}`, `{hit_at}`, `{resets_at}`, `{resets_at_local}`,
/// `{raw_match}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitHook {
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub url_env: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hitl {
    #[serde(default = "default_sensitive_actions")]
    pub globally_sensitive_actions: Vec<String>,
    #[serde(default)]
    pub auto_approve_windows: Vec<AutoApprove>,
}

impl Default for Hitl {
    fn default() -> Self {
        Self {
            globally_sensitive_actions: default_sensitive_actions(),
            auto_approve_windows: Vec::new(),
        }
    }
}

fn default_sensitive_actions() -> Vec<String> {
    vec![
        "publish".into(),
        "release".into(),
        "payment".into(),
        "external_email".into(),
        "external_api_post".into(),
        "merge_to_main".into(),
        "dns_change".into(),
        "deploy".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoApprove {
    pub action: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// RFC 3339 timestamp in UTC.
    pub until: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRef {
    pub file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Broker {
    #[serde(default = "default_broker_type")]
    pub r#type: String,
    #[serde(default = "default_mailbox_path")]
    pub path: PathBuf,
}

impl Default for Broker {
    fn default() -> Self {
        Self {
            r#type: default_broker_type(),
            path: default_mailbox_path(),
        }
    }
}

fn default_broker_type() -> String {
    "sqlite".into()
}

fn default_mailbox_path() -> PathBuf {
    PathBuf::from("state/mailbox.db")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorCfg {
    #[serde(default = "default_supervisor_type")]
    pub r#type: String,
    #[serde(default = "default_tmux_prefix")]
    pub tmux_prefix: String,
    /// Seconds reload waits for an agent to exit gracefully after
    /// SIGINT before falling through to a hard `kill-session`. Default
    /// 10 — enough for an in-flight Claude Code tool call to finish
    /// in the common case, short enough that operators don't sit
    /// staring at a frozen reload. Set to 0 to disable graceful
    /// drain (matches pre-PR-B hard-kill behaviour).
    #[serde(default = "default_drain_timeout_secs")]
    pub drain_timeout_secs: u64,
    /// Per-session worktree isolation. When `true`, every agent's
    /// tmux session launches in its own git worktree under
    /// `<root>/state/worktrees/<agent>/` on its own
    /// `agents/<agent-id>` branch, so concurrent file mutations
    /// across sessions don't collide. When `false`, every agent
    /// shares the project's `cwd` (legacy single-cwd behaviour).
    ///
    /// Field absent → treated as `false` (legacy) at runtime so
    /// existing pre-v2-A teams upgrade without their tmux sessions
    /// silently moving. New teams scaffolded by `teamctl init` write
    /// `worktree_isolation: true` explicitly; pre-v2-A teams see a
    /// one-time validate warning nudging opt-in. Per-agent
    /// `cwd_override` opts a single agent out regardless of this flag.
    #[serde(default)]
    pub worktree_isolation: Option<bool>,
}

impl SupervisorCfg {
    /// Effective worktree-isolation flag for callers. Absent → `false`
    /// (legacy single-cwd behaviour) so existing teams upgrade
    /// without their tmux sessions silently moving directories. New
    /// teams scaffolded by `teamctl init` write `worktree_isolation:
    /// true` explicitly; pre-v2-A teams get a one-time validate
    /// warning prompting opt-in. Industry-standard
    /// deprecate→warn→opt-in→next-major-flips cadence.
    pub fn worktree_isolation_enabled(&self) -> bool {
        self.worktree_isolation.unwrap_or(false)
    }
}

impl Default for SupervisorCfg {
    fn default() -> Self {
        Self {
            r#type: default_supervisor_type(),
            tmux_prefix: default_tmux_prefix(),
            drain_timeout_secs: default_drain_timeout_secs(),
            worktree_isolation: None,
        }
    }
}

fn default_supervisor_type() -> String {
    "tmux".into()
}

fn default_drain_timeout_secs() -> u64 {
    10
}

fn default_tmux_prefix() -> String {
    "a-".into()
}

/// Per-project file, e.g. `projects/hello.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub project: ProjectMeta,

    #[serde(default)]
    pub channels: Vec<Channel>,

    #[serde(default)]
    pub managers: BTreeMap<String, Agent>,

    #[serde(default)]
    pub workers: BTreeMap<String, Agent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    /// Either a list of agent ids or the literal string `"*"`.
    #[serde(default)]
    pub members: ChannelMembers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChannelMembers {
    All(String),
    Explicit(Vec<String>),
}

impl Default for ChannelMembers {
    fn default() -> Self {
        Self::Explicit(Vec::new())
    }
}

impl ChannelMembers {
    pub fn includes(&self, agent: &str, all_agents: &[&str]) -> bool {
        match self {
            ChannelMembers::All(s) if s == "*" => all_agents.contains(&agent),
            ChannelMembers::Explicit(v) => v.iter().any(|a| a == agent),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    #[serde(default = "default_runtime")]
    pub runtime: String,
    pub model: Option<String>,
    pub role_prompt: Option<PathBuf>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default = "default_autonomy")]
    pub autonomy: String,
    #[serde(default)]
    pub can_dm: Vec<String>,
    #[serde(default)]
    pub can_broadcast: Vec<String>,
    #[serde(default)]
    pub reports_to: Option<String>,

    /// Override the global rate-limit hook chain for this agent.
    #[serde(default)]
    pub on_rate_limit: Option<Vec<String>>,

    /// Per-agent reasoning effort. Renders as `EFFORT=<value>` in the
    /// agent env file; the wrapper passes it to the runtime (e.g.
    /// `claude --effort <value>`). Strict enum: typos like `hgih` fail
    /// compose validation rather than silently falling back to the
    /// wrapper default.
    #[serde(default)]
    pub effort: Option<EffortLevel>,

    /// Per-manager human-facing interfaces. Today's only adapter is
    /// `telegram`; the shape is reserved for future adapters
    /// (`discord`, `imessage`, …) so a manager can declare every
    /// channel it speaks on in one place. Workers leave this unset.
    #[serde(default)]
    pub interfaces: Option<AgentInterfaces>,

    /// Opt this agent out of per-session worktree isolation. When set,
    /// the agent's tmux session launches with this path as `cwd`
    /// instead of the auto-derived `<root>/state/worktrees/<agent>/`.
    /// Advanced use — lets an operator plug an externally-managed
    /// worktree (e.g. a long-lived feature branch) into a session.
    /// Relative paths resolve against the compose root.
    #[serde(default)]
    pub cwd_override: Option<PathBuf>,
}

/// Container for per-manager interface adapters. Open shape so adding
/// `discord:` / `imessage:` later is a strictly-additive YAML edit.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInterfaces {
    /// 1:1 Telegram bot for this manager. When set, `teamctl up`
    /// spawns a `team-bot` tmux session scoped to this manager so the
    /// human DMs the bot directly (no `/dm role text` required).
    /// Configured by `teamctl bot setup`.
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
}

/// Per-manager Telegram bot config. Both fields are env-var *names* —
/// the actual token/chat-ids live in `.team/.env` (kept out of git).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Env var holding the BotFather token. Default chosen by
    /// `teamctl bot setup`: `TEAMCTL_TG_<MANAGER>_TOKEN`.
    pub bot_token_env: String,
    /// Env var holding a comma-separated list of authorized chat ids.
    /// Default: `TEAMCTL_TG_<MANAGER>_CHATS`.
    pub chat_ids_env: String,
}

impl Agent {
    /// Convenience: pull the manager's Telegram config out of
    /// `interfaces.telegram` without forcing every callsite to handle
    /// the nested options.
    pub fn telegram(&self) -> Option<&TelegramConfig> {
        self.interfaces.as_ref().and_then(|i| i.telegram.as_ref())
    }
}

/// Reasoning-effort level forwarded to the runtime. Maps 1:1 to
/// `claude --effort <value>` today; if the runtime taxonomy evolves we
/// extend the enum and bump the schema version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl EffortLevel {
    /// Lowercase rendering for the env-file `EFFORT=<value>` line and
    /// the `claude --effort <value>` CLI flag.
    pub fn as_str(self) -> &'static str {
        match self {
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
            EffortLevel::Xhigh => "xhigh",
            EffortLevel::Max => "max",
        }
    }
}

fn default_runtime() -> String {
    "claude-code".into()
}

fn default_autonomy() -> String {
    "low_risk_only".into()
}

/// Fully loaded compose tree: global + resolved projects.
#[derive(Debug, Clone)]
pub struct Compose {
    pub root: PathBuf,
    pub global: Global,
    pub projects: Vec<Project>,
}

impl Compose {
    /// Walk up from `start` looking for the **first** `.team/team-compose.yaml`
    /// and return the directory containing the compose file (the "root"),
    /// suitable for passing to [`Compose::load`]. The first hit wins; we do
    /// not keep walking past it to look for a parent `.team/`.
    ///
    /// This is the equivalent of git's `.git/` discovery — once a repo carries
    /// a `.team/` folder, every `teamctl` subcommand finds it from anywhere
    /// inside the tree. T-008 retired the legacy flat-layout fallback and
    /// the second-hit / parent-`.team/` walk: the convention is `.team/` and
    /// the nearest one wins, no exceptions.
    pub fn discover(start: &Path) -> anyhow::Result<PathBuf> {
        let start = start
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("canonicalize {}: {e}", start.display()))?;
        let mut cur: Option<&Path> = Some(&start);
        while let Some(dir) = cur {
            let candidate = dir.join(".team").join("team-compose.yaml");
            if candidate.is_file() {
                return Ok(dir.join(".team"));
            }
            cur = dir.parent();
        }
        Err(anyhow::anyhow!(
            "no `.team/team-compose.yaml` found in {} or any parent",
            start.display()
        ))
    }

    /// Parse `team-compose.yaml` at `root` and every referenced project file.
    pub fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let global_path = root.join("team-compose.yaml");
        let global: Global = serde_yaml::from_str(
            &std::fs::read_to_string(&global_path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", global_path.display()))?,
        )
        .map_err(|e| anyhow::anyhow!("parse {}: {e}", global_path.display()))?;

        let mut projects = Vec::with_capacity(global.projects.len());
        for r in &global.projects {
            let p = root.join(&r.file);
            let parsed: Project = serde_yaml::from_str(
                &std::fs::read_to_string(&p)
                    .map_err(|e| anyhow::anyhow!("read {}: {e}", p.display()))?,
            )
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", p.display()))?;
            projects.push(parsed);
        }

        Ok(Self {
            root,
            global,
            projects,
        })
    }

    /// Resolve the per-agent cwd given current schema settings.
    ///
    /// Resolution order:
    /// 1. `agent.cwd_override` — explicit per-agent opt-out wins.
    /// 2. Worktree isolation enabled (default) → `<root>/state/worktrees/<agent>/`.
    /// 3. Fallback → the project's resolved `cwd` (back-compat).
    ///
    /// Relative `cwd_override` and `project.cwd` paths resolve against
    /// the compose root.
    pub fn resolve_agent_cwd(&self, h: &AgentHandle) -> PathBuf {
        if let Some(o) = &h.spec.cwd_override {
            return if o.is_absolute() {
                o.clone()
            } else {
                self.root.join(o)
            };
        }
        if self.global.supervisor.worktree_isolation_enabled() {
            return self.root.join("state/worktrees").join(h.agent);
        }
        let project_cwd = self
            .projects
            .iter()
            .find(|p| p.project.id == h.project)
            .map(|p| &p.project.cwd);
        match project_cwd {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => self.root.join(p),
            None => self.root.clone(),
        }
    }

    /// Resolve the project's source-of-truth cwd (the directory git
    /// worktree commands run against). The agent worktrees are derived
    /// off this; for tests + edge-case validation we need it standalone.
    pub fn resolve_project_cwd(&self, project_id: &str) -> Option<PathBuf> {
        self.projects
            .iter()
            .find(|p| p.project.id == project_id)
            .map(|p| {
                if p.project.cwd.is_absolute() {
                    p.project.cwd.clone()
                } else {
                    self.root.join(&p.project.cwd)
                }
            })
    }

    /// Return every agent in the compose tree tagged with manager/worker.
    pub fn agents(&self) -> impl Iterator<Item = AgentHandle<'_>> {
        self.projects.iter().flat_map(|p| {
            p.managers
                .iter()
                .map(move |(id, a)| AgentHandle {
                    project: &p.project.id,
                    agent: id,
                    spec: a,
                    is_manager: true,
                })
                .chain(p.workers.iter().map(move |(id, a)| AgentHandle {
                    project: &p.project.id,
                    agent: id,
                    spec: a,
                    is_manager: false,
                }))
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AgentHandle<'a> {
    pub project: &'a str,
    pub agent: &'a str,
    pub spec: &'a Agent,
    pub is_manager: bool,
}

impl AgentHandle<'_> {
    /// Canonical id as `<project>:<agent>`.
    pub fn id(&self) -> String {
        format!("{}:{}", self.project, self.agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_members_all_expands() {
        let all = ChannelMembers::All("*".into());
        assert!(all.includes("dev1", &["dev1", "dev2"]));
        assert!(!all.includes("ghost", &["dev1", "dev2"]));
    }

    #[test]
    fn channel_members_explicit_checks_list() {
        let exp = ChannelMembers::Explicit(vec!["dev1".into(), "critic".into()]);
        assert!(exp.includes("dev1", &[]));
        assert!(!exp.includes("dev2", &[]));
    }

    #[test]
    fn agent_defaults_are_stable() {
        let a: Agent = serde_yaml::from_str("model: claude-opus-4-7\n").unwrap();
        assert_eq!(a.runtime, "claude-code");
        assert_eq!(a.autonomy, "low_risk_only");
        assert!(a.interfaces.is_none());
        assert!(a.telegram().is_none());
        assert!(a.effort.is_none());
        assert!(a.cwd_override.is_none());
    }

    #[test]
    fn agent_cwd_override_parses() {
        let a: Agent =
            serde_yaml::from_str("runtime: claude-code\ncwd_override: ./custom\n").unwrap();
        assert_eq!(a.cwd_override, Some(PathBuf::from("./custom")));
    }

    #[test]
    fn supervisor_worktree_isolation_parses() {
        let s: SupervisorCfg =
            serde_yaml::from_str("type: tmux\nworktree_isolation: true\n").unwrap();
        assert_eq!(s.worktree_isolation, Some(true));
        assert!(s.worktree_isolation_enabled());

        let absent: SupervisorCfg = serde_yaml::from_str("type: tmux\n").unwrap();
        assert_eq!(absent.worktree_isolation, None);
        assert!(
            !absent.worktree_isolation_enabled(),
            "absent defaults to legacy single-cwd (false) per pm-ratified opt-in semantics"
        );

        let off: SupervisorCfg =
            serde_yaml::from_str("type: tmux\nworktree_isolation: false\n").unwrap();
        assert!(!off.worktree_isolation_enabled());
    }

    #[test]
    fn resolve_agent_cwd_honors_override_isolation_and_fallback() {
        let mut managers = BTreeMap::new();
        let agent_with_override = Agent {
            runtime: "claude-code".into(),
            cwd_override: Some(PathBuf::from("custom-pane")),
            ..base_agent()
        };
        let agent_isolated = Agent {
            runtime: "claude-code".into(),
            ..base_agent()
        };
        managers.insert("override_pm".into(), agent_with_override);
        managers.insert("isolated_pm".into(), agent_isolated);

        let mut compose = make_compose(managers);
        compose.root = PathBuf::from("/repo/.team");
        compose.projects[0].project.cwd = PathBuf::from("..");

        // Case 1: cwd_override wins regardless of isolation flag.
        compose.global.supervisor.worktree_isolation = Some(true);
        let h = compose.agents().find(|a| a.agent == "override_pm").unwrap();
        assert_eq!(
            compose.resolve_agent_cwd(&h),
            PathBuf::from("/repo/.team/custom-pane")
        );

        // Case 2: isolation true (default), no override → state/worktrees/<agent>.
        let h = compose.agents().find(|a| a.agent == "isolated_pm").unwrap();
        assert_eq!(
            compose.resolve_agent_cwd(&h),
            PathBuf::from("/repo/.team/state/worktrees/isolated_pm")
        );

        // Case 3: isolation false → fall through to project cwd.
        compose.global.supervisor.worktree_isolation = Some(false);
        let h = compose.agents().find(|a| a.agent == "isolated_pm").unwrap();
        assert_eq!(
            compose.resolve_agent_cwd(&h),
            PathBuf::from("/repo/.team/..")
        );

        // Case 4: isolation absent → legacy single-cwd (matches false).
        compose.global.supervisor.worktree_isolation = None;
        let h = compose.agents().find(|a| a.agent == "isolated_pm").unwrap();
        assert_eq!(
            compose.resolve_agent_cwd(&h),
            PathBuf::from("/repo/.team/..")
        );
    }

    fn base_agent() -> Agent {
        Agent {
            runtime: "claude-code".into(),
            model: None,
            role_prompt: None,
            permission_mode: None,
            autonomy: "low_risk_only".into(),
            can_dm: vec![],
            can_broadcast: vec![],
            reports_to: None,
            on_rate_limit: None,
            effort: None,
            interfaces: None,
            cwd_override: None,
        }
    }

    fn make_compose(managers: BTreeMap<String, Agent>) -> Compose {
        Compose {
            root: PathBuf::from("/teamctl"),
            global: Global {
                version: 2,
                broker: Default::default(),
                supervisor: Default::default(),
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
            },
            projects: vec![Project {
                version: 2,
                project: ProjectMeta {
                    id: "proj".into(),
                    name: "Proj".into(),
                    cwd: PathBuf::from("."),
                },
                channels: vec![],
                managers,
                workers: BTreeMap::default(),
            }],
        }
    }

    #[test]
    fn agent_telegram_block_parses_under_interfaces() {
        let yaml = "interfaces:\n  telegram:\n    bot_token_env: T\n    chat_ids_env: C\n";
        let a: Agent = serde_yaml::from_str(yaml).unwrap();
        let tg = a.telegram().expect("telegram parsed");
        assert_eq!(tg.bot_token_env, "T");
        assert_eq!(tg.chat_ids_env, "C");
    }

    #[test]
    fn effort_parses_all_five_levels() {
        for (yaml, expected) in [
            ("effort: low\n", EffortLevel::Low),
            ("effort: medium\n", EffortLevel::Medium),
            ("effort: high\n", EffortLevel::High),
            ("effort: xhigh\n", EffortLevel::Xhigh),
            ("effort: max\n", EffortLevel::Max),
        ] {
            let a: Agent = serde_yaml::from_str(yaml).expect(yaml);
            assert_eq!(a.effort, Some(expected), "yaml: {yaml}");
        }
    }

    #[test]
    fn effort_unknown_value_is_rejected() {
        let err = serde_yaml::from_str::<Agent>("effort: hgih\n")
            .expect_err("typo'd effort value must fail to parse");
        let msg = err.to_string();
        assert!(
            msg.contains("low") && msg.contains("max"),
            "error should enumerate valid variants; got: {msg}"
        );
    }

    #[test]
    fn effort_renders_to_lowercase_string() {
        assert_eq!(EffortLevel::Low.as_str(), "low");
        assert_eq!(EffortLevel::Xhigh.as_str(), "xhigh");
        assert_eq!(EffortLevel::Max.as_str(), "max");
    }

    #[test]
    fn discover_prefers_dot_team() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join(".team")).unwrap();
        std::fs::write(repo.join(".team/team-compose.yaml"), "version: 2\n").unwrap();
        // a stray flat-layout file in the same dir should NOT be preferred.
        std::fs::write(repo.join("team-compose.yaml"), "version: 2\n").unwrap();

        // Walking up from a sub-dir should still find the .team/ root.
        let sub = repo.join("src/deep/nested");
        std::fs::create_dir_all(&sub).unwrap();
        let found = Compose::discover(&sub).unwrap();
        assert_eq!(found, repo.canonicalize().unwrap().join(".team"));
    }

    #[test]
    fn discover_no_longer_falls_back_to_flat_layout() {
        // T-008: a flat `team-compose.yaml` at cwd (no `.team/` wrapper) is
        // not discoverable. The convention is `.team/`. Operators must
        // either `init` a `.team/` or pass `--root` explicitly.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("team-compose.yaml"), "version: 2\n").unwrap();
        let err = Compose::discover(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("no `.team/team-compose.yaml`"));
    }

    #[test]
    fn discover_returns_first_dot_team_walking_up() {
        // T-008 boundary: nested `.team/`s win over outer ones. We do NOT
        // keep walking past the first hit.
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path();
        let inner = outer.join("packages/inner");
        std::fs::create_dir_all(outer.join(".team")).unwrap();
        std::fs::write(outer.join(".team/team-compose.yaml"), "version: 2\n").unwrap();
        std::fs::create_dir_all(inner.join(".team")).unwrap();
        std::fs::write(inner.join(".team/team-compose.yaml"), "version: 2\n").unwrap();

        let from_inner = inner.join("src/deep");
        std::fs::create_dir_all(&from_inner).unwrap();
        let found = Compose::discover(&from_inner).unwrap();
        assert_eq!(found, inner.canonicalize().unwrap().join(".team"));
    }

    #[test]
    fn discover_errors_when_nothing_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Compose::discover(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("no `.team/team-compose.yaml`"));
    }
}
