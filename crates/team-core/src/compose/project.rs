use super::*;

/// Per-project file, e.g. `projects/hello.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub version: u32,
    pub project: ProjectMeta,

    #[serde(default)]
    pub channels: Vec<Channel>,

    #[serde(default)]
    pub managers: BTreeMap<String, Agent>,

    #[serde(default)]
    pub workers: BTreeMap<String, Agent>,

    /// Project-scoped human-facing interfaces (#132 PR-1). Mirrors the
    /// per-agent `Agent.interfaces` shape one level up — `telegram` is
    /// today's only adapter, with room for future `discord:` /
    /// `imessage:` under the same `ProjectInterfaces` container. Hosts
    /// the shared bot-family config (manager bot for managed-bots flow,
    /// profile-picture defaults) that's scoped to one project's bot
    /// family but spawns N per-agent children — not per-agent because
    /// it's shared infra, not global because each project deserves its
    /// own bot-family identity. Absent → existing manual BotFather
    /// per-manager flow runs verbatim (zero-touch).
    #[serde(default)]
    pub interfaces: Option<ProjectInterfaces>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Channel {
    pub name: String,
    /// Either a list of agent ids or the literal string `"*"`.
    #[serde(default)]
    pub members: ChannelMembers,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// One per-agent Claude Code hook declared in compose (#383 Phase 2).
///
/// Maps onto Claude Code's `settings.json` hook shape: an `event` bucket
/// (`PreToolUse`, `PostToolUse`, `Stop`, …) holding entries of
/// `{ matcher, hooks: [{ type: "command", command }] }`. teamctl does not
/// enumerate the runtime's event names — `event` is passed through
/// verbatim so a new Claude Code event works without a teamctl release.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HookSpec {
    /// Claude Code hook event the command fires on, e.g. `PreToolUse`.
    pub event: String,

    /// Optional tool-name regex (`Bash`, `Edit|Write`). Omitted → the
    /// hook matches every tool for the event, matching Claude Code's own
    /// behavior when `matcher` is absent.
    #[serde(default)]
    pub matcher: Option<String>,

    /// Compose-root-relative path to the hook command, resolved the same
    /// way as `role_prompt: roles/x.md` and rendered into the settings
    /// file as an absolute path. v1 is a single executable path (no
    /// inline args) — matching the issue's `command: hooks/guard.sh`
    /// shape.
    pub command: PathBuf,
}

/// One per-agent MCP server declared in compose (#383 Phase 4).
///
/// Serializes straight into the runtime's MCP config entry — `command` /
/// `args` / `env` map onto the same shape the built-in `team` server
/// emits. The HTTP (`url` / `headers`) transport variant is deferred
/// (spike E2) until a concrete need lands.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpServer {
    /// Executable that launches the MCP server, resolved on `$PATH` by
    /// the runtime (e.g. `npx`, `docker`).
    pub command: String,

    /// Arguments passed to `command`. Empty (the default) → `[]`.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables for the server process. Values pass through
    /// verbatim — render does no interpolation, matching how teamctl
    /// treats env elsewhere. `${VAR}` placeholders are expanded by Claude
    /// Code at launch; codex does NOT interpolate them (validate warns —
    /// see `ValidationWarning::McpEnvInterpolationUnsupported`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    /// #461: per-agent ultracode opt-in. When `true`, teamctl emits
    /// `"ultracode": true` into the agent's Claude Code settings JSON (the
    /// file the wrapper passes via `--settings`); the resulting session
    /// behavior is Claude Code's, not teamctl's. ultracode is a Claude Code
    /// *setting* — not an effort level and not a CLI flag — so it's
    /// orthogonal to `effort:` above (which rides the `--effort` flag) and
    /// the two compose freely. Like the `enableAllProjectMcpServers` key in
    /// `render_claude_settings`, the name is vendor-owned: verified against
    /// Claude Code 2.1.175, and a future rename would silently no-op.
    /// claude-only v1: on non-claude runtimes the settings file is skipped
    /// and a declared opt-in warns (codex/gemini have no equivalent).
    /// Defaults to `false` → the settings JSON is byte-unchanged for agents
    /// that don't opt in.
    #[serde(default)]
    pub ultracode: bool,

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

    /// #383 Phase 2: per-agent Claude Code hooks, merged additively into
    /// the per-agent `settings.json` that render already builds, on top
    /// of the built-in interactive-prompt deny hook (which keeps
    /// precedence — see `render::render_claude_settings`). Commands are
    /// compose-root-relative paths, resolved like `role_prompt`.
    /// claude-only v1: declared on a non-`claude-code` agent they render
    /// nothing and render logs an "unsupported runtime" warning. Empty
    /// (the default) → settings unchanged.
    #[serde(default)]
    pub hooks: Vec<HookSpec>,

    /// #383 Phase 4: per-agent MCP servers, merged into the rendered
    /// per-agent MCP config alongside the built-in `team` mailbox server
    /// (which stays unconditional and non-clobberable — a declared server
    /// named `team` is rejected at validate and skipped at render). Unlike
    /// hooks, MCP is runtime-agnostic: declared servers render for every
    /// runtime whose descriptor sets `supports_mcp`, and are skipped with
    /// a warning otherwise. Empty (the default) → MCP config unchanged.
    #[serde(default)]
    pub mcps: BTreeMap<String, McpServer>,

    /// #383 Phase 3a: per-agent Claude Code sub-agents declared in compose.
    /// Each entry is a compose-root-relative path to a standard sub-agent
    /// markdown file (frontmatter `name` / `description` / optional `tools`
    /// / `model`, body → the sub-agent's system prompt), resolved like
    /// `role_prompt`. render transforms the list into Claude Code's
    /// `--agents` inline JSON so each agent gets its own sub-agents
    /// additively, on top of the project `.claude/agents/` and the built-in
    /// sub-agents (verified: `--agents` adds, never replaces), without an
    /// arbitrary-path flag (the only cwd-stationary mechanism — see the
    /// Phase-1 spike). claude-only v1: declared on a non-`claude-code`
    /// agent they render nothing and render logs an "unsupported runtime"
    /// warning. Empty (the default) → no `--agents` flag.
    #[serde(default)]
    pub subagents: Vec<PathBuf>,

    /// #383 Phase 3b: per-agent Claude Code skills declared in compose.
    /// Each entry is a compose-root-relative path to a skill directory
    /// (the folder holding `SKILL.md`), resolved like `role_prompt`. render
    /// materializes a per-agent scope dir under
    /// `state/agent-scope/<project>-<agent>/.claude/skills/` holding a
    /// symlink to each declared skill, and the wrapper passes that scope
    /// dir via `claude --add-dir` so the agent discovers its skills
    /// additively, on top of the project `.claude/skills/` (verified:
    /// `--add-dir` adds, never replaces — see the Phase-1 spike §9-E1).
    /// claude-only v1: declared on a non-`claude-code` agent they
    /// materialize nothing and render logs an "unsupported runtime"
    /// warning. Empty (the default) → no `--add-dir` flag.
    #[serde(default)]
    pub skills: Vec<PathBuf>,
}

/// Container for per-manager interface adapters. Open shape so adding
/// `discord:` / `imessage:` later is a strictly-additive YAML edit.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// #132 PR-1: project-scoped interface container. One level up from
/// `AgentInterfaces`, same open-shape rationale — today's only adapter
/// is `telegram`, future `discord:` / `imessage:` slot in as
/// strictly-additive YAML edits.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ProjectInterfaces {
    /// Project-scoped Telegram config: the manager bot that spawns
    /// per-agent children + default profile-picture rendering. Per-
    /// agent telegram config (under `Agent.interfaces.telegram`) is
    /// orthogonal — agents still declare their own `bot_token_env` /
    /// `chat_ids_env` slots; the managed-bots flow writes child tokens
    /// into those slots untouched.
    #[serde(default)]
    pub telegram: Option<ProjectTelegramConfig>,
}

/// #132 PR-1: project-scoped Telegram config. Hosts the shared-infra
/// fields (manager bot, profile-picture defaults) that drive the
/// managed-bots flow in `teamctl bot setup`. Absent / both fields
/// absent → existing manual BotFather walkthrough runs verbatim for
/// each per-agent `Agent.interfaces.telegram` block (zero-touch).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ProjectTelegramConfig {
    /// Manager bot for the managed-bots flow. When set + its env var
    /// resolves, `teamctl bot setup` uses it to spawn per-agent child
    /// bots via Telegram's Bot API 10.0 managed-bot endpoints. Absent
    /// → operator runs the manual BotFather flow per agent as today.
    #[serde(default)]
    pub manager_bot: Option<ManagerBotConfig>,

    /// Default profile-picture rendering for spawned child bots.
    /// Image-model path is opt-in; absent / failure-of-generation
    /// falls back to deterministic initials-in-colored-circle (Q3-
    /// ratified). When the whole block is absent, no profile-picture
    /// is applied — child bots keep Telegram's default avatar.
    #[serde(default)]
    pub profile_picture: Option<ProfilePictureConfig>,
}

/// #132 PR-1: manager-bot config. Mirrors the env-var-name pattern of
/// `TelegramConfig.bot_token_env` — the actual BotFather token lives
/// in `.team/.env`, this field names the env var that holds it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManagerBotConfig {
    /// Env var holding the manager bot's BotFather token. The
    /// operator-facing setup steps (BotFather click path to enable the
    /// Managed Bots capability) are documented in `docs/`; this schema
    /// pins the env-var name the wizard reads at setup time.
    pub token_env: String,
}

/// #132 PR-1: profile-picture rendering settings for spawned child
/// bots. `image_model` is opt-in for AI-generated avatars; `fallback`
/// names the rendering used when `image_model` is absent OR generation
/// fails OR the API key is missing. Q3 (owner-ratified, tg 3445):
/// initials-in-colored-circle, no embedded emoji-font binary growth.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ProfilePictureConfig {
    /// AI image-generation config. When set + API key resolves, the
    /// wizard generates a square 512×512 image seeded from the agent's
    /// name + role and applies it via Bot API `setProfilePhoto`. Any
    /// failure path (missing API key, generation error, upload error)
    /// falls through to `fallback`.
    #[serde(default)]
    pub image_model: Option<ImageModelConfig>,

    /// Fallback rendering when `image_model` is absent or fails. v1
    /// has one variant (`Initials`); the field is explicit so future
    /// variants (per-agent override, `None`-to-skip) slot in
    /// additively without breaking existing YAML.
    #[serde(default)]
    pub fallback: ProfilePictureFallback,
}

/// #132 PR-1: profile-picture fallback rendering. Q3-ratified to
/// initials-in-colored-circle for v1. Enum-shaped so future variants
/// (e.g. `Emoji`, `None`) land additively; current single variant is
/// the default so omitting `fallback:` in YAML keeps the contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProfilePictureFallback {
    /// Render a deterministic colored circle with the agent's
    /// uppercase initials (Slack-style). Deterministic = same agent
    /// name always renders the same circle, so rebuilds don't shuffle.
    #[default]
    Initials,
}

/// #132 PR-1: AI image-generation config for child-bot profile
/// pictures. Mirrors the `SttConfig` shape (provider / api_key_env /
/// model). v1 provider is `openai`; adding a future provider is one
/// match arm in the call site.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageModelConfig {
    /// Provider arm. v1: `openai`.
    pub provider: String,
    /// Env var holding the provider's API key (mirrors `bot_token_env`
    /// and `SttConfig.api_key_env`). The actual secret lives in
    /// `.team/.env` and is resolved by `teamctl bot setup` at wizard
    /// time.
    pub api_key_env: String,
    /// Provider model id (e.g. `gpt-image-2` for OpenAI; snapshot id
    /// `gpt-image-2-2026-04-21` for pinning).
    pub model: String,
}

impl Project {
    /// Convenience: pull the project's Telegram config out of
    /// `interfaces.telegram` without forcing every callsite to handle
    /// the nested options. Mirrors [`Agent::telegram`].
    pub fn telegram(&self) -> Option<&ProjectTelegramConfig> {
        self.interfaces.as_ref().and_then(|i| i.telegram.as_ref())
    }
}

/// Reasoning-effort level forwarded to the runtime. Maps 1:1 to
/// `claude --effort <value>` today; if the runtime taxonomy evolves we
/// extend the enum and bump the schema version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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
