//! YAML schema for `team-compose.yaml` and `projects/<id>.yaml`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

/// T-265 PR-a: compose schema version. Stored as a semver string;
/// validate-time check (`validate::validate`) enforces the semver
/// shape via the `semver` crate.
///
/// **Custom Deserialize accepts two shapes:**
///
/// - YAML string (e.g. `version: "2.0.0"`) — taken verbatim; semver
///   shape is checked later at validate time, NOT here, so the
///   deserializer's job stays narrow (parse, not validate).
/// - YAML integer literal `2` only (the one legacy value that ever
///   shipped in any in-tree compose) → coerced to `SchemaVersion`
///   carrying `"2.0.0"` AND flagged `from_legacy_int = true`. The
///   load orchestration in [`Compose::load`] reads that flag to
///   decide whether to auto-rewrite the on-disk file so the
///   integer self-heals to the semver shape (owner-ratified tg
///   2989 + tg 3440, "option 1 + variant A").
///
/// Anything else — `version: 1`, `version: 3`, `version: true`,
/// `version: [1,2,3]` — fails to deserialize with a message that
/// names the constraint: only `"X.Y.Z"` or the legacy `2`.
///
/// `from_legacy_int` is `#[serde(skip)]` so it never round-trips
/// through serialize; it's a deserialize-side signal only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion {
    pub value: String,
    #[serde(skip)]
    pub from_legacy_int: bool,
}

impl SchemaVersion {
    /// Construct directly from a semver-shaped string; for fixtures
    /// and tests + the in-memory legacy coercion.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            from_legacy_int: false,
        }
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = SchemaVersion;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a semver string like \"2.0.0\" (legacy integer `2` also accepted \
                     and auto-rewritten to \"2.0.0\" on next save)",
                )
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<SchemaVersion, E> {
                Ok(SchemaVersion {
                    value: s.to_string(),
                    from_legacy_int: false,
                })
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<SchemaVersion, E> {
                Ok(SchemaVersion {
                    value: s,
                    from_legacy_int: false,
                })
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<SchemaVersion, E> {
                if n == 2 {
                    Ok(SchemaVersion {
                        value: "2.0.0".to_string(),
                        from_legacy_int: true,
                    })
                } else {
                    Err(E::custom(format!(
                        "compose schema version must be a semver string like \"2.0.0\"; \
                         got integer {n} — only legacy `2` is auto-coerced"
                    )))
                }
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<SchemaVersion, E> {
                if n == 2 {
                    Ok(SchemaVersion {
                        value: "2.0.0".to_string(),
                        from_legacy_int: true,
                    })
                } else {
                    Err(E::custom(format!(
                        "compose schema version must be a semver string like \"2.0.0\"; \
                         got integer {n} — only legacy `2` is auto-coerced"
                    )))
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// Top-level `team-compose.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Global {
    pub version: SchemaVersion,

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

    /// T-32 file attachments. Optional — omit the entire block to get
    /// default behavior (enabled, 5MB cap, `$HOME` allowed root, no
    /// scanner, no audit log). Each field is also optional.
    #[serde(default)]
    pub attachments: Attachments,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachments {
    #[serde(default = "default_attachments_enabled")]
    pub enabled: bool,
    #[serde(default = "default_attachments_max_size_bytes")]
    pub max_size_bytes: u64,
    /// Roots the attachment path must be a descendant of after
    /// canonicalization. Default is the operator's `$HOME` (resolved
    /// at policy-check time, not at deserialize time, so a snapshot
    /// taken on machine A still resolves correctly on machine B).
    #[serde(default = "default_attachments_allowed_roots")]
    pub allowed_roots: Vec<String>,
    #[serde(default)]
    pub scanner: Option<AttachmentScanner>,
    /// When set, every attempt is appended to this file (path,
    /// sha256, size, accept/reject, scanner stderr). Relative paths
    /// resolve against the compose root.
    #[serde(default)]
    pub audit_log_path: Option<PathBuf>,
    /// T-32b: TTL in seconds for staged tempfiles in
    /// `state/attachments-staging/`. The agent's `read_attachment`
    /// MCP tool returns a staging path; the file lives until the TTL
    /// expires (sweep on team-mcp startup) or the operator explicitly
    /// persists it via a future tool. Default 6h gives an LLM
    /// session enough room to round-trip without the staging dir
    /// bloating indefinitely.
    #[serde(default = "default_attachments_tempfile_ttl_seconds")]
    pub tempfile_ttl_seconds: u64,
}

impl Default for Attachments {
    fn default() -> Self {
        Self {
            enabled: default_attachments_enabled(),
            max_size_bytes: default_attachments_max_size_bytes(),
            allowed_roots: default_attachments_allowed_roots(),
            scanner: None,
            audit_log_path: None,
            tempfile_ttl_seconds: default_attachments_tempfile_ttl_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentScanner {
    /// Operator-provided executable. Spawned per attempt with the
    /// resolved path as a single argument; non-zero exit → reject.
    pub command: String,
    #[serde(default = "default_scanner_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_attachments_enabled() -> bool {
    true
}

fn default_attachments_max_size_bytes() -> u64 {
    5 * 1024 * 1024
}

fn default_attachments_allowed_roots() -> Vec<String> {
    vec!["$HOME".to_string()]
}

fn default_scanner_timeout_seconds() -> u64 {
    30
}

fn default_attachments_tempfile_ttl_seconds() -> u64 {
    6 * 60 * 60
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
}

impl Default for SupervisorCfg {
    fn default() -> Self {
        Self {
            r#type: default_supervisor_type(),
            tmux_prefix: default_tmux_prefix(),
            drain_timeout_secs: default_drain_timeout_secs(),
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

/// Reference to one or more role-instruction markdown files.
///
/// Single-string form (current) keeps every existing compose parsing
/// unchanged. List form lets a role compose from multiple files
/// concatenated in declared order at boot — base + tweaks without
/// duplicating shared role copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RolePrompt {
    Single(PathBuf),
    Multiple(Vec<PathBuf>),
}

impl RolePrompt {
    /// All source paths in declared order. Single yields a one-element
    /// slice; Multiple yields the list as-is.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            RolePrompt::Single(p) => vec![p.as_path()],
            RolePrompt::Multiple(v) => v.iter().map(|p| p.as_path()).collect(),
        }
    }

    /// True when the configured value resolves to no actual source
    /// path: an empty string in the single form, or an empty list in
    /// the multi form. Renderer would silently produce
    /// `SYSTEM_PROMPT_PATH=<root>/` otherwise — caught at validate.
    pub fn is_blank(&self) -> bool {
        match self {
            RolePrompt::Single(p) => p.as_os_str().is_empty(),
            RolePrompt::Multiple(v) => v.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    #[serde(default = "default_runtime")]
    pub runtime: String,
    pub model: Option<String>,
    pub role_prompt: Option<RolePrompt>,
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

    /// T-160: optional human-friendly label rendered by the TUI in
    /// place of the agent id (roster, details header, mailbox row
    /// attribution, statusline). Absent → render the agent id (current
    /// behavior). Validation: non-empty, ≤64 chars, UTF-8 anything.
    /// The agent id stays canonical for routing, tmux session names,
    /// CLI args, and YAML cross-refs (`can_dm`, `can_broadcast`,
    /// `reports_to`) — display_name is render-time only.
    #[serde(default)]
    pub display_name: Option<String>,
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
    /// Optional speech-to-text provider for voice messages. When set,
    /// inbound Telegram voice notes are transcribed and forwarded to the
    /// agent prefixed so the model knows the input came from audio.
    /// Absent → voice messages stay unhandled (default).
    #[serde(default)]
    pub speech_to_text: Option<SttConfig>,
}

/// Speech-to-text settings for the per-manager Telegram bot. The provider
/// arm is the only switch v1 needs (`groq`); adding OpenAI Whisper or
/// whisper.cpp later is one match arm in `team-bot`'s transcribe function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    /// Provider arm. v1: `groq`.
    pub provider: String,
    /// Env var holding the provider's API key (mirrors `bot_token_env`).
    /// The actual secret lives in `.team/.env` and is resolved by
    /// `teamctl bot up` at spawn time before being passed to `team-bot`.
    pub api_key_env: String,
    /// Provider model id (e.g. `whisper-large-v3` for Groq).
    pub model: String,
    /// Optional ISO-639 language hint forwarded verbatim to the provider
    /// (e.g. `en`, `fa`). When unset, the provider auto-detects.
    #[serde(default)]
    pub language: Option<String>,
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
        let raw = std::fs::read_to_string(&global_path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", global_path.display()))?;
        let global: Global = serde_yaml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", global_path.display()))?;

        // T-265 PR-a: legacy-`2` auto-rewrite on load. When the
        // operator's compose still uses the pre-semver shape
        // (`version: 2`, integer literal), the Deserialize impl on
        // `SchemaVersion` has already coerced the in-memory value to
        // `"2.0.0"` and flagged `from_legacy_int = true`. Now we
        // best-effort rewrite the file so the on-disk shape matches
        // the runtime semantics — eliminating the file-vs-runtime
        // divergence the operator would otherwise see in git diff
        // forever. On RO filesystems (CI sandboxes, immutable image
        // mounts) or any other write failure, we emit a single warn
        // and proceed with the in-memory normalized value rather
        // than hard-erroring — owner-ratified (tg 3440, "RO-FS
        // degrades to in-memory + warning"). Single hardcoded
        // legacy-value exception, NOT the general migration engine
        // — that stays deferred to its own ticket per #265's
        // non-goals.
        if global.version.from_legacy_int {
            if let Err(e) = rewrite_legacy_version_in_file(&global_path, &raw) {
                tracing::warn!(
                    target: "team-core::compose",
                    "could not rewrite legacy `version: 2` in {}: {e}; \
                     proceeding with in-memory `\"2.0.0\"`",
                    global_path.display()
                );
            }
        }

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

/// T-265 PR-a: rewrite the legacy `version: 2` integer literal in
/// `team-compose.yaml` to the semver string form `"2.0.0"`,
/// preserving comments + key ordering via the `yaml_edit` substrate.
/// Caller has already loaded the raw text (passed as `raw` to avoid
/// a second disk read) and decided we're in the legacy path.
fn rewrite_legacy_version_in_file(path: &Path, raw: &str) -> anyhow::Result<()> {
    let updated = crate::yaml_edit::set_top_level_scalar(raw, "version", "\"2.0.0\"")?;
    std::fs::write(path, updated).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
    Ok(())
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
        let a: Agent = serde_yaml::from_str("model: claude-opus-4-8\n").unwrap();
        assert_eq!(a.runtime, "claude-code");
        assert_eq!(a.autonomy, "low_risk_only");
        assert!(a.interfaces.is_none());
        assert!(a.telegram().is_none());
        assert!(a.effort.is_none());
    }

    #[test]
    fn agent_telegram_block_parses_under_interfaces() {
        let yaml = "interfaces:\n  telegram:\n    bot_token_env: T\n    chat_ids_env: C\n";
        let a: Agent = serde_yaml::from_str(yaml).unwrap();
        let tg = a.telegram().expect("telegram parsed");
        assert_eq!(tg.bot_token_env, "T");
        assert_eq!(tg.chat_ids_env, "C");
        assert!(tg.speech_to_text.is_none());
    }

    #[test]
    fn agent_telegram_block_parses_speech_to_text() {
        let yaml = "\
interfaces:
  telegram:
    bot_token_env: T
    chat_ids_env: C
    speech_to_text:
      provider: groq
      api_key_env: GROQ_API_KEY
      model: whisper-large-v3
      language: en
";
        let a: Agent = serde_yaml::from_str(yaml).unwrap();
        let stt = a
            .telegram()
            .and_then(|t| t.speech_to_text.as_ref())
            .expect("speech_to_text parsed");
        assert_eq!(stt.provider, "groq");
        assert_eq!(stt.api_key_env, "GROQ_API_KEY");
        assert_eq!(stt.model, "whisper-large-v3");
        assert_eq!(stt.language.as_deref(), Some("en"));
    }

    #[test]
    fn agent_telegram_block_parses_speech_to_text_without_language() {
        let yaml = "\
interfaces:
  telegram:
    bot_token_env: T
    chat_ids_env: C
    speech_to_text:
      provider: groq
      api_key_env: K
      model: whisper-large-v3
";
        let a: Agent = serde_yaml::from_str(yaml).unwrap();
        let stt = a
            .telegram()
            .and_then(|t| t.speech_to_text.as_ref())
            .expect("speech_to_text parsed");
        assert!(stt.language.is_none());
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

    #[test]
    fn role_prompt_parses_single_string_form() {
        let yaml = "role_prompt: roles/mgr.md\n";
        let agent: Agent = serde_yaml::from_str(&format!(
            "runtime: claude-code\nautonomy: low_risk_only\n{yaml}"
        ))
        .unwrap();
        match agent.role_prompt.unwrap() {
            RolePrompt::Single(p) => assert_eq!(p, PathBuf::from("roles/mgr.md")),
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn role_prompt_parses_list_form() {
        let yaml = "role_prompt:\n  - roles/_base.md\n  - roles/mgr.md\n";
        let agent: Agent = serde_yaml::from_str(&format!(
            "runtime: claude-code\nautonomy: low_risk_only\n{yaml}"
        ))
        .unwrap();
        match agent.role_prompt.unwrap() {
            RolePrompt::Multiple(v) => assert_eq!(
                v,
                vec![
                    PathBuf::from("roles/_base.md"),
                    PathBuf::from("roles/mgr.md"),
                ]
            ),
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn role_prompt_paths_returns_declared_order() {
        let rp = RolePrompt::Multiple(vec![
            PathBuf::from("a.md"),
            PathBuf::from("b.md"),
            PathBuf::from("c.md"),
        ]);
        let got: Vec<&Path> = rp.paths();
        assert_eq!(
            got,
            vec![Path::new("a.md"), Path::new("b.md"), Path::new("c.md")]
        );
    }

    // T-265 PR-a: SchemaVersion Deserialize semantics. The owner-
    // ratified contract is: accept YAML string verbatim; accept the
    // single legacy integer `2` (coerce to "2.0.0", flag
    // `from_legacy_int = true` so Compose::load knows to rewrite
    // the file); reject anything else with a message naming the
    // constraint.

    #[test]
    fn schema_version_accepts_semver_string() {
        let v: SchemaVersion = serde_yaml::from_str("\"2.0.0\"").unwrap();
        assert_eq!(v.value, "2.0.0");
        assert!(!v.from_legacy_int, "string form is NOT the legacy path");
    }

    #[test]
    fn schema_version_accepts_arbitrary_semver_string_for_later_validation() {
        // Deserialize doesn't enforce the semver shape — validate
        // does. So `"abc"` parses fine here; the validate-time check
        // rejects it later. Test pins this contract — keeps the
        // deserialize impl narrow.
        let v: SchemaVersion = serde_yaml::from_str("\"abc\"").unwrap();
        assert_eq!(v.value, "abc");
    }

    #[test]
    fn schema_version_coerces_legacy_integer_two() {
        let v: SchemaVersion = serde_yaml::from_str("2").unwrap();
        assert_eq!(v.value, "2.0.0");
        assert!(v.from_legacy_int, "integer-2 must be flagged for rewrite");
    }

    #[test]
    fn schema_version_rejects_other_integers() {
        // The hardcoded-legacy-exception is EXACTLY `2`. Anything
        // else (`1`, `3`, `99`) hard-errors with a message naming
        // the constraint.
        for n in [0u64, 1, 3, 99] {
            let err = serde_yaml::from_str::<SchemaVersion>(&n.to_string())
                .expect_err("non-2 integer must fail");
            let msg = err.to_string();
            assert!(
                msg.contains("only legacy `2` is auto-coerced"),
                "error must name the constraint; got: {msg}"
            );
        }
    }

    #[test]
    fn schema_version_rejects_non_string_non_int_shapes() {
        // Booleans, lists, mappings — none of them are a version.
        for yaml in ["true", "[1,2,3]", "{a: b}"] {
            let res = serde_yaml::from_str::<SchemaVersion>(yaml);
            assert!(res.is_err(), "yaml `{yaml}` must fail to deserialize");
        }
    }

    /// T-265 PR-a: Compose::load orchestration test — legacy `2`
    /// file gets auto-rewritten to the semver string AND the
    /// in-memory representation is `"2.0.0"` + `from_legacy_int =
    /// true` (the flag the load logic reads to decide whether to
    /// rewrite). The on-disk content after load must be `version:
    /// "2.0.0"`, comments preserved.
    #[test]
    fn load_rewrites_legacy_version_two_in_file_and_in_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".team");
        std::fs::create_dir_all(&root).unwrap();
        let yaml = "\
# T-265 fixture — legacy version
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
";
        std::fs::write(root.join("team-compose.yaml"), yaml).unwrap();
        let compose = Compose::load(&root).expect("load succeeds on legacy file");
        // In-memory: normalized + flagged legacy.
        assert_eq!(compose.global.version.value, "2.0.0");
        // On-disk: rewritten to the semver string.
        let after = std::fs::read_to_string(root.join("team-compose.yaml")).unwrap();
        assert!(
            after.contains("version: \"2.0.0\""),
            "file must be rewritten;\n{after}"
        );
        assert!(
            !after.contains("\nversion: 2\n"),
            "no legacy literal must survive;\n{after}"
        );
        // Comment preserved.
        assert!(
            after.contains("# T-265 fixture"),
            "comment must survive the rewrite;\n{after}"
        );
        // broker block survives.
        assert!(after.contains("type: sqlite"));
    }

    #[test]
    fn load_leaves_canonical_semver_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".team");
        std::fs::create_dir_all(&root).unwrap();
        let yaml = "\
version: \"2.0.0\"
broker:
  type: sqlite
";
        std::fs::write(root.join("team-compose.yaml"), yaml).unwrap();
        let compose = Compose::load(&root).expect("load succeeds");
        assert_eq!(compose.global.version.value, "2.0.0");
        assert!(
            !compose.global.version.from_legacy_int,
            "canonical file must NOT be flagged for rewrite"
        );
        // File content byte-identical (no auto-rewrite when not legacy).
        let after = std::fs::read_to_string(root.join("team-compose.yaml")).unwrap();
        assert_eq!(after, yaml, "canonical file must NOT be mutated on load");
    }

    #[test]
    fn load_hard_errors_on_non_two_integer_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".team");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("team-compose.yaml"), "version: 3\n").unwrap();
        let err = Compose::load(&root).expect_err("must reject integer-3 at parse");
        assert!(
            err.to_string().contains("only legacy `2` is auto-coerced"),
            "error must name the constraint; got: {err}"
        );
    }
}
