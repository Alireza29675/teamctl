use super::*;

/// Top-level `team-compose.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Interface {
    /// Adapter type: `telegram`, `discord`, `imessage`, `cli`, `webhook`, ...
    pub r#type: String,
    /// Free-form name; used in logs and to route approvals.
    pub name: String,
    /// Adapter-specific config (bot token, channel id, allowlist, …).
    // T-265: serde_yaml::Value has no JsonSchema impl; render it as
    // arbitrary JSON via serde_json::Value (which does), since the
    // adapter config is intentionally free-form.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRef {
    pub file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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
