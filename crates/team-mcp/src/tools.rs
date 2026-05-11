//! MCP tool definitions and dispatch.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::store::Store;
use std::path::PathBuf;
use team_core::attachments::Scanner;
use team_core::compose::Attachments;

pub struct Ctx {
    pub agent_id: String,
    pub store: Arc<Store>,
    /// Tmux session prefix (matches `compose.global.supervisor.tmux_prefix`).
    /// Used by `compact_self` to compute the caller's tmux session name
    /// when sending the `/compact` slash command into its pane.
    pub tmux_prefix: String,
    /// T-32b: attachment policy + compose root. `None` when team-mcp
    /// was launched without `--compose-root` (hand-launched servers,
    /// older renderer); `read_attachment` returns "disabled" in that
    /// case so an unconfigured server doesn't silently expose the
    /// filesystem.
    pub attachments: Option<AttachmentsCtx>,
}

/// Bundles the attachment-related state on `Ctx` so the tool body
/// stays parameter-light. The scanner is constructed once at boot
/// and re-used across calls.
pub struct AttachmentsCtx {
    pub cfg: Attachments,
    pub compose_root: PathBuf,
    pub scanner: Option<Box<dyn Scanner>>,
}

impl Ctx {
    pub fn new(agent_id: String, store: Store, tmux_prefix: String) -> Self {
        Self {
            agent_id,
            store: Arc::new(store),
            tmux_prefix,
            attachments: None,
        }
    }

    pub fn with_attachments(mut self, attachments: AttachmentsCtx) -> Self {
        self.attachments = Some(attachments);
        self
    }

    pub fn project(&self) -> &str {
        self.agent_id.split(':').next().unwrap_or("")
    }
}

/// Convenience for tests: build a `Ctx` with an explicit compose
/// root + attachments cfg. Production wiring goes through `main.rs`.
#[cfg(test)]
impl Ctx {
    pub fn for_test_with_attachments(
        agent_id: String,
        store: Store,
        compose_root: &std::path::Path,
        cfg: Attachments,
        scanner: Option<Box<dyn Scanner>>,
    ) -> Self {
        Self::new(agent_id, store, "t-".into()).with_attachments(AttachmentsCtx {
            cfg,
            compose_root: compose_root.to_path_buf(),
            scanner,
        })
    }
}

/// JSON-Schema-ish tool list for `tools/list`.
pub fn schema() -> Value {
    json!([
        {
            "name": "whoami",
            "description": "Return the caller's fully-qualified agent id.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "dm",
            "description": "Send a direct message to another agent (same project). Returns the new message id.",
            "inputSchema": {
                "type": "object",
                "required": ["to", "text"],
                "properties": {
                    "to":        { "type": "string", "description": "Target agent id. Either `<project>:<agent>` or a bare `<agent>` in the caller's project." },
                    "text":      { "type": "string" },
                    "thread_id": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_peek",
            "description": "Return up to `limit` unacked messages addressed to the caller. Non-destructive — peek does not mark anything resolved. Use this to browse the queue; use `inbox_read` to commit to handling specific ids (which fetches + auto-acks).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_read",
            "description": "Fetch full bodies for the listed message ids and mark them resolved in the same call (T-104: read-is-resolve). The default channel notification is a stub; call `inbox_read` with the stub's `meta.id` to drill in. Ids the caller can't see, or already-acked ids, are silently skipped.",
            "inputSchema": {
                "type": "object",
                "required": ["ids"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_ack",
            "description": "Mark the listed message ids as acknowledged without reading the body — use to dismiss messages you've decided not to handle. To handle (read + resolve), use `inbox_read` instead.",
            "inputSchema": {
                "type": "object",
                "required": ["ids"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_watch",
            "description": "Block up to `timeout_ms` milliseconds waiting for a new message. Returns immediately if any are pending.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 60000, "default": 15000 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "broadcast",
            "description": "Post a message to a channel in the caller's project. Caller must be a channel member and have the channel listed in can_broadcast.",
            "inputSchema": {
                "type": "object",
                "required": ["channel", "text"],
                "properties": {
                    "channel": { "type": "string" },
                    "text":    { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "list_team",
            "description": "List every agent in the caller's project (project-scoped; never returns other projects).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "org_chart",
            "description": "Return the project's org chart: managers (top tier) and workers with their `reports_to` links. Use to introspect who is above you.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "reply_to_user",
            "description": "Send a message to the human operator. Available only to managers (`is_manager: true`); the configured interface adapter (Telegram, Discord, …) forwards it. \n\nThis is the ONLY channel back to the human — anything you write outside this tool is invisible to them. Use it to answer their DMs, surface progress on long-running work, escalate blockers, or proactively share something they should know. Do NOT use `dm` for human traffic (it is project-scoped inter-agent). \n\nFor work that takes more than a minute, send a brief acknowledgement first (e.g. \"on it — checking the build\") and then a separate reply when done; do not leave the operator wondering whether you started. \n\nAttach an `image` (jpg/png/webp/gif, ≤50MB) or `file` (any type, ≤50MB) by passing `{source: \"path\"|\"url\", value: \"<path or URL>\", caption?: \"<short caption>\"}`. Each of `text`, `image`, `file` lands as its own chat message; combine them in one call to send a screenshot with a follow-up sentence in a single tool invocation. At least one of `text`, `image`, `file` is required.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":      {
                        "type": "string",
                        "description": "Mostly plain text. A small markdown subset is rendered: `**bold**`, `__bold__`, `*italic*`, `` `code` ``, and triple-backtick fenced code blocks (with optional language tag). Single-underscore italic (`_text_`) is intentionally not rendered — underscore is too common in code-style text. Bullet lines (`- item` / `* item` / `+ item`) become `• item`. Plain `<`, `>`, `&` are safe — escaped automatically. No headings (Telegram does not render them). Use emojis sparingly to aid scanability (✅ done, ⚠️ caution, 🔧 working, ❓ question). Aim for short, chat-sized messages; split long output into multiple calls rather than sending a wall of text."
                    },
                    "image": {
                        "type": "object",
                        "description": "Image attachment. Sources: `path` (absolute path on the manager's machine) or `url` (publicly fetchable). Allowed types: jpg/jpeg/png/webp/gif. Path-source files must be ≤50MB.",
                        "required": ["source", "value"],
                        "properties": {
                            "source":  { "type": "string", "enum": ["path", "url"] },
                            "value":   { "type": "string", "description": "Absolute filesystem path or public URL." },
                            "caption": { "type": "string", "description": "Optional caption rendered under the photo. Plain text, ≤1024 chars per Telegram." }
                        },
                        "additionalProperties": false
                    },
                    "file": {
                        "type": "object",
                        "description": "File attachment. Same sources as `image`, no mime restriction beyond Telegram's own. Path-source files must be ≤50MB.",
                        "required": ["source", "value"],
                        "properties": {
                            "source":  { "type": "string", "enum": ["path", "url"] },
                            "value":   { "type": "string" },
                            "caption": { "type": "string" }
                        },
                        "additionalProperties": false
                    },
                    "thread_id": {
                        "type": "string",
                        "description": "Optional. Group this reply with an existing conversation thread. Pass the `thread_id` value you saw in the channel meta of the inbound message you are responding to; omit for a fresh thread."
                    },
                    "reply_to_message_id": {
                        "type": "integer",
                        "description": "Optional. Mailbox id of the inbound user message you're replying to — pass the `meta.id` value from the channel envelope (or `inbox_peek.id`). The bot resolves it to the right Telegram message id at insert time and threads the reply below the operator's message in the chat client. Omit when sending a fresh message. Applies to all of `text` / `image` / `file` in this call — they all reply to the same parent."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "request_approval",
            "description": "Request human approval for a brand-sensitive action. Blocks until approved/denied/expired/undeliverable (long-poll). Use before any tool call that publishes, deploys, pays, or sends externally. Terminal `undeliverable` means the prompt was never marked delivered to a human surface — distinct from `expired` (delivered but no decision in time).",
            "inputSchema": {
                "type": "object",
                "required": ["action", "summary"],
                "properties": {
                    "action":     { "type": "string", "description": "Coarse category, e.g. publish, deploy, payment." },
                    "scope_tag":  { "type": "string", "description": "Optional narrower tag for auto-approval matching." },
                    "summary":    { "type": "string" },
                    "payload":    { "type": "object" },
                    "ttl_seconds":{ "type": "integer", "minimum": 30, "maximum": 3600, "default": 900 },
                    "wait":       { "type": "boolean", "default": true, "description": "When false, return immediately after inserting the row (status=pending, delivered_at=null). Useful for diagnostics and non-blocking flows." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "show_typing",
            "description": "Show a Telegram \"typing…\" indicator in the operator's chat. Available only to managers (`is_manager: true`). Use right before kicking off work that takes more than a moment to produce visible output, so the operator gets a social cue that you're working rather than staring at silence. The indicator clears the moment any text from `reply_to_user` reaches the chat, or after a 10-second ceiling — whichever comes first. Calling `show_typing` again within an active window extends the ceiling (resets the 10s clock). No-op-safe: calling repeatedly is fine.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "read_attachment",
            "description": "Read a file the operator attached to a message. The bot/CLI surfaces attachments as a body line `📎 attachment: <absolute-path>`; pass that path here to receive a staging tempfile path the agent can `read_file()` directly. The broker enforces three guards: the path must canonicalize beneath one of `attachments.allowed_roots` (default `[$HOME]`), the file must be ≤ `max_size_bytes` (default 5 MB), and a configured `attachments.scanner` must return clean. Rejected reads return `{rejected: true, reason: \"…\"}` and never expose bytes. Operator gets a notification on telegram (when configured) and in the project's TUI Wire tab when the broker rejects. Staged tempfiles live in `<compose-root>/state/attachments-staging/` and are cleaned on team-mcp startup beyond the configured `tempfile_ttl_seconds` (default 6 h).",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path the operator passed in `📎 attachment: <path>`. Symlinks and relative paths are accepted but the broker canonicalizes before policy checks."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "compact_self",
            "description": "Compact your own context window via Claude Code's `/compact` command. Available on `claude-code` runtimes only (other runtimes don't recognize the slash command). \n\n**This is destructive: prior conversation detail is summarized and irretrievably trimmed.** Use only when explicitly instructed by your role (e.g. \"compact after every completed task\") or when you have clearly finished a major chunk of work and want to free space for the next one. Do not call this casually — every call permanently loses turns from your working window. \n\nFire-and-forget: the call returns immediately, and the `/compact` slash command lands in your tmux pane within a few milliseconds. Compaction itself runs asynchronously inside your session. The tool only routes; it does not block on the compaction completing.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "react_to_user",
            "description": "Apply an emoji reaction to a specific Telegram message from the operator. Available only to managers (`is_manager: true`). Use to acknowledge an inbound DM lightly without sending a full reply — 👀 to signal you're on it, ✍ to signal you're typing, 👍 to ack done. Each `react_to_user` call replaces any previous bot reaction on that message; pass an unsupported emoji and the call rejects with a clear error before reaching Telegram. The set of allowed emoji is the standard Telegram bot-reaction set (premium-tier-agnostic, ~75 emoji); use what you'd reach for in normal chat reactions. Pass the `telegram_msg_id` value from the inbound mailbox row you're reacting to.",
            "inputSchema": {
                "type": "object",
                "required": ["telegram_msg_id", "emoji"],
                "properties": {
                    "telegram_msg_id": {
                        "type": "integer",
                        "description": "Telegram message id to react to. Pass the `telegram_msg_id` value from the inbound mailbox row you're acknowledging."
                    },
                    "emoji": {
                        "type": "string",
                        "description": "Reaction emoji. Must be one of the allowed bot-reaction emojis (👍 👎 ❤️ 🔥 🥰 👏 😁 🤔 🤯 😱 🤬 😢 🎉 🤩 🤮 💩 🙏 👌 🕊 🤡 🥱 🥴 😍 🐳 💯 🤣 ⚡ 🍌 🏆 💔 🤨 😐 🍓 🍾 💋 🖕 😈 😴 😭 🤓 👻 👀 🎃 🙈 😇 😨 🤝 ✍ 🤗 🫡 🎅 🎄 ☃ 💅 🤪 🗿 🆒 💘 🙉 🦄 😘 💊 🙊 😎 👾 🤷 😡 plus a few combos like ❤️‍🔥 🌚 🌭 👨‍💻). Out-of-set emoji rejected at the MCP boundary."
                    }
                },
                "additionalProperties": false
            }
        }
    ])
}

#[derive(Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

pub async fn call(ctx: &Ctx, params: Value) -> Result<Value, String> {
    let p: CallParams = serde_json::from_value(params).map_err(|e| e.to_string())?;
    match p.name.as_str() {
        "whoami" => Ok(content_text(&ctx.agent_id)),
        "dm" => dm(ctx, p.arguments).await,
        "inbox_peek" => inbox_peek(ctx, p.arguments),
        "inbox_read" => inbox_read(ctx, p.arguments),
        "inbox_ack" => inbox_ack(ctx, p.arguments),
        "inbox_watch" => inbox_watch(ctx, p.arguments).await,
        "broadcast" => broadcast(ctx, p.arguments),
        "list_team" => list_team(ctx),
        "org_chart" => org_chart(ctx),
        "request_approval" => request_approval(ctx, p.arguments).await,
        "reply_to_user" => reply_to_user(ctx, p.arguments).await,
        "react_to_user" => react_to_user(ctx, p.arguments).await,
        "show_typing" => show_typing(ctx).await,
        "read_attachment" => read_attachment(ctx, p.arguments).await,
        "compact_self" => compact_self(ctx).await,
        other => Err(format!("unknown tool: {other}")),
    }
}

fn content_text(s: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": s } ], "isError": false })
}

fn content_json(v: &Value) -> Value {
    json!({
        "content": [
            { "type": "text", "text": serde_json::to_string(v).unwrap_or_default() }
        ],
        "isError": false,
        "structuredContent": v,
    })
}

#[derive(Deserialize)]
struct DmArgs {
    to: String,
    text: String,
    #[serde(default)]
    thread_id: Option<String>,
}

async fn dm(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: DmArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    // Resolve bare `<agent>` as `<self-project>:<agent>`.
    let recipient = if a.to.contains(':') {
        a.to.clone()
    } else {
        format!("{}:{}", ctx.project(), a.to)
    };
    // Project isolation: DM recipient must be in the same project as caller.
    let caller_project = ctx.project().to_string();
    let recipient_project = recipient.split(':').next().unwrap_or_default().to_string();
    if recipient_project != caller_project {
        // Cross-project: only allowed when a live bridge authorizes it.
        match ctx
            .store
            .live_bridge(&ctx.agent_id, &recipient)
            .map_err(|e| e.to_string())?
        {
            Some(_bridge_id) => {
                // Permitted. Thread-id is used by `teamctl bridge log` to
                // reconstruct the transcript.
            }
            None => {
                return Err(format!(
                    "project isolation: cannot DM across projects ({caller_project} -> {recipient_project}); open a bridge",
                ));
            }
        }
    }
    // ACL: `can_dm` must include the recipient (or be empty = unrestricted).
    if !ctx
        .store
        .can_dm(&ctx.agent_id, &recipient)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {sender} is not permitted to DM {recipient}",
            sender = ctx.agent_id
        ));
    }
    // If this is a bridged DM, record the bridge id in thread_id for auditing.
    let bridge_thread = if recipient_project != caller_project {
        ctx.store
            .live_bridge(&ctx.agent_id, &recipient)
            .ok()
            .flatten()
            .map(|id| format!("bridge:{id}"))
    } else {
        None
    };
    let thread_id = bridge_thread.as_deref().or(a.thread_id.as_deref());
    let id = ctx
        .store
        .send_dm(
            &caller_project,
            &ctx.agent_id,
            &recipient,
            &a.text,
            thread_id,
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "recipient": recipient })))
}

#[derive(Deserialize)]
struct ReplyToUserArgs {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    image: Option<MediaArg>,
    #[serde(default)]
    file: Option<MediaArg>,
    #[serde(default)]
    thread_id: Option<String>,
    /// Mailbox id of the inbound user message to thread under (T-168).
    /// Agents pass the `meta.id` they see in the channel envelope; the
    /// store resolves it to the row's Telegram message id at insert time
    /// (`resolve_telegram_msg_id` in `store.rs`) and persists that for the
    /// bot's outbound dispatcher to feed `reply_parameters` with. Applies
    /// to all of `text` / `image` / `file` for this call — they all
    /// visually nest under the same parent message.
    #[serde(default)]
    reply_to_message_id: Option<i64>,
}

#[derive(Deserialize)]
struct MediaArg {
    source: MediaSource,
    value: String,
    #[serde(default)]
    caption: Option<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MediaSource {
    Path,
    Url,
}

/// Per-file size cap matching Telegram's bot-API ceiling for photo/document
/// uploads. URLs bypass the local check — Telegram will validate on its end.
const MEDIA_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Image extensions Telegram's `sendPhoto` reliably renders. We accept the
/// caller's claim by extension; sniffing magic bytes would be more rigorous
/// but the failure mode (Telegram rejects misnamed file) surfaces a
/// recoverable error rather than data loss, so the cheap check earns its
/// place over the expensive one.
fn image_extension_allowed(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Validate a `path`-source media descriptor: file must exist, be ≤50MB,
/// and (for images) carry an allowlisted extension. URL-source descriptors
/// skip these checks — neither the size nor the mime is knowable without
/// fetching, and Telegram performs both checks server-side anyway.
fn validate_media(kind: &str, m: &MediaArg) -> Result<(), String> {
    if matches!(m.source, MediaSource::Url) {
        return Ok(());
    }
    let meta = std::fs::metadata(&m.value)
        .map_err(|e| format!("reply_to_user: {kind} path not readable ({}): {e}", m.value))?;
    if !meta.is_file() {
        return Err(format!(
            "reply_to_user: {kind} path is not a regular file: {}",
            m.value
        ));
    }
    if meta.len() > MEDIA_MAX_BYTES {
        return Err(format!(
            "reply_to_user: {kind} too large ({} bytes); 50MB cap per file",
            meta.len()
        ));
    }
    if kind == "image" && !image_extension_allowed(&m.value) {
        return Err(format!(
            "reply_to_user: image extension not in allowlist (jpg/jpeg/png/webp/gif): {}",
            m.value
        ));
    }
    Ok(())
}

fn payload_json(m: &MediaArg) -> String {
    let source = match m.source {
        MediaSource::Path => "path",
        MediaSource::Url => "url",
    };
    let mut payload = json!({ "source": source, "value": m.value });
    if let Some(caption) = &m.caption {
        payload["caption"] = json!(caption);
    }
    payload.to_string()
}

async fn reply_to_user(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ReplyToUserArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    if !ctx
        .store
        .is_manager(&ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "reply_to_user: only managers can reply to the user (caller={})",
            ctx.agent_id
        ));
    }
    let text_present = a.text.as_deref().is_some_and(|t| !t.is_empty());
    if !text_present && a.image.is_none() && a.file.is_none() {
        return Err("reply_to_user: at least one of `text`, `image`, `file` must be set".into());
    }
    if let Some(m) = &a.image {
        validate_media("image", m)?;
    }
    if let Some(m) = &a.file {
        validate_media("file", m)?;
    }
    let project = ctx.project().to_string();
    let recipient = "user:telegram";
    let thread = a.thread_id.as_deref();
    let reply_to = a.reply_to_message_id;

    let mut ids: Vec<i64> = Vec::with_capacity(3);
    if text_present {
        let id = ctx
            .store
            .send_dm(
                &project,
                &ctx.agent_id,
                recipient,
                a.text.as_deref().unwrap_or(""),
                thread,
                reply_to,
            )
            .map_err(|e| e.to_string())?;
        ids.push(id);
    }
    if let Some(m) = &a.image {
        let id = ctx
            .store
            .send_dm_kind(
                &project,
                &ctx.agent_id,
                recipient,
                m.caption.as_deref().unwrap_or(""),
                thread,
                "image",
                &payload_json(m),
                reply_to,
            )
            .map_err(|e| e.to_string())?;
        ids.push(id);
    }
    if let Some(m) = &a.file {
        let id = ctx
            .store
            .send_dm_kind(
                &project,
                &ctx.agent_id,
                recipient,
                m.caption.as_deref().unwrap_or(""),
                thread,
                "file",
                &payload_json(m),
                reply_to,
            )
            .map_err(|e| e.to_string())?;
        ids.push(id);
    }
    // Back-compat: keep the legacy `id` field (= first inserted id) so
    // existing text-only callers still see the same response shape.
    let first = ids.first().copied().unwrap_or(0);
    Ok(content_json(
        &json!({ "id": first, "ids": ids, "recipient": recipient }),
    ))
}

/// Allowed bot-reaction emoji per Telegram's free-tier `setMessageReaction`
/// allowlist (PHASE-1 §3.5). Premium-tier bots get a wider set; we don't
/// assume premium so we mirror Telegram's free-tier allowlist verbatim.
/// Out-of-set emoji are rejected at the MCP boundary so the agent sees a
/// clean error rather than a Telegram API rejection three layers down. The
/// allowlist constant is the single source of truth — if Telegram extends
/// the set in a future bot-API release, refresh here.
const BOT_REACTION_ALLOWLIST: &[&str] = &[
    "👍",
    "👎",
    "❤️",
    "🔥",
    "🥰",
    "👏",
    "😁",
    "🤔",
    "🤯",
    "😱",
    "🤬",
    "😢",
    "🎉",
    "🤩",
    "🤮",
    "💩",
    "🙏",
    "👌",
    "🕊",
    "🤡",
    "🥱",
    "🥴",
    "😍",
    "🐳",
    "❤️‍🔥",
    "🌚",
    "🌭",
    "💯",
    "🤣",
    "⚡",
    "🍌",
    "🏆",
    "💔",
    "🤨",
    "😐",
    "🍓",
    "🍾",
    "💋",
    "🖕",
    "😈",
    "😴",
    "😭",
    "🤓",
    "👻",
    "👨‍💻",
    "👀",
    "🎃",
    "🙈",
    "😇",
    "😨",
    "🤝",
    "✍️",
    "🤗",
    "🫡",
    "🎅",
    "🎄",
    "☃️",
    "💅",
    "🤪",
    "🗿",
    "🆒",
    "💘",
    "🙉",
    "🦄",
    "😘",
    "💊",
    "🙊",
    "😎",
    "👾",
    "🤷‍♂️",
    "🤷",
    "🤷‍♀️",
    "😡",
];

fn is_allowed_reaction(emoji: &str) -> bool {
    BOT_REACTION_ALLOWLIST.contains(&emoji)
}

#[derive(Deserialize)]
struct ReactToUserArgs {
    telegram_msg_id: i64,
    emoji: String,
}

async fn react_to_user(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ReactToUserArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    if !ctx
        .store
        .is_manager(&ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "react_to_user: only managers can react to the user (caller={})",
            ctx.agent_id
        ));
    }
    if !is_allowed_reaction(&a.emoji) {
        return Err(format!(
            "react_to_user: emoji `{}` is not in the bot-reaction allowlist; \
             pick one of the supported reactions (see schema description).",
            a.emoji
        ));
    }
    let project = ctx.project().to_string();
    let recipient = "user:telegram";
    // Reaction rows ride the existing T-086-A `kind`+`structured_payload`
    // discriminator. The bot's outbound dispatcher reads `kind = "reaction"`
    // and routes to `setMessageReaction` instead of `sendMessage`. The
    // `text` column carries the emoji as a fallback for legacy readers
    // (e.g. if a non-Telegram interface adapter ever needs to render
    // reactions as inline text).
    let payload = json!({
        "telegram_msg_id": a.telegram_msg_id,
        "emoji": a.emoji,
    })
    .to_string();
    let id = ctx
        .store
        .send_dm_kind(
            &project,
            &ctx.agent_id,
            recipient,
            &a.emoji,
            None,
            "reaction",
            &payload,
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(
        &json!({ "id": id, "recipient": recipient, "telegram_msg_id": a.telegram_msg_id, "emoji": a.emoji }),
    ))
}

/// T-102: open or extend a Telegram "typing…" window for the operator's
/// chat. Like `reply_to_user` and `react_to_user`, the actual Telegram
/// call happens in `team-bot`; this side just appends a discriminator
/// row to the mailbox. The bot's outbound dispatcher reads
/// `kind = "typing"` and refreshes `sendChatAction` until either a
/// text/image/file row from the same agent path lands (which clears the
/// window) or a 10-second ceiling expires.
async fn show_typing(ctx: &Ctx) -> Result<Value, String> {
    if !ctx
        .store
        .is_manager(&ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "show_typing: only managers can show typing (caller={})",
            ctx.agent_id
        ));
    }
    let project = ctx.project().to_string();
    let recipient = "user:telegram";
    let id = ctx
        .store
        .send_dm_kind(
            &project,
            &ctx.agent_id,
            recipient,
            "",
            None,
            "typing",
            "{}",
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "recipient": recipient })))
}

/// T-32b: read an operator-attached file via the broker policy.
/// The agent passes the path it found in a message body's
/// `📎 attachment: <path>` marker; the broker canonicalizes,
/// path-traversal-checks against `allowed_roots`, size-checks
/// against `max_size_bytes`, runs the configured scanner if any,
/// stages the bytes to a content-addressed tempfile, audit-logs the
/// attempt, and returns the staging path. Rejects fan out a
/// notification both to telegram (`recipient = 'user:telegram'`)
/// and to the project's `all` channel (TUI Wire tab) so the
/// operator sees the reason regardless of which surface they're on.
#[derive(Deserialize)]
struct ReadAttachmentArgs {
    path: String,
}

async fn read_attachment(ctx: &Ctx, args: Value) -> Result<Value, String> {
    use std::path::PathBuf;
    use team_core::attachments::{
        append_audit, check_and_read_with_metadata, now_rfc3339, stage_to_tempfile, staging_dir,
        AuditEntry, RejectReason,
    };

    let a: ReadAttachmentArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let raw_path = PathBuf::from(&a.path);

    // Without compose context we can't enforce policy or stage.
    // Hand-launched team-mcp instances see this — surface as a
    // disabled-style reject so the agent doesn't think they're
    // getting bytes.
    let Some(att) = ctx.attachments.as_ref() else {
        return Ok(content_json(&json!({
            "rejected": true,
            "reason": "attachments unavailable: team-mcp launched without --compose-root",
        })));
    };

    let cfg = &att.cfg;
    let scanner_ref: Option<&dyn team_core::attachments::Scanner> = att.scanner.as_deref();

    let outcome = check_and_read_with_metadata(cfg, &raw_path, scanner_ref);
    let audit_path = cfg
        .audit_log_path
        .as_ref()
        .map(|p| resolve_audit_path(&att.compose_root, p));

    match outcome {
        Ok(accepted) => {
            let staged = stage_to_tempfile(&staging_dir(&att.compose_root), &accepted)
                .map_err(|e| format!("stage_to_tempfile: {e}"))?;
            // Audit on accept.
            let entry = AuditEntry {
                ts: now_rfc3339(),
                path: &a.path,
                resolved: accepted.resolved.to_str(),
                outcome: "accept",
                size: Some(accepted.size),
                blake3: Some(&accepted.blake3_hex),
                reason: None,
            };
            if let Err(e) = append_audit(audit_path.as_deref(), &entry) {
                tracing::warn!(error = %e, "audit log append failed (accept)");
            }
            Ok(content_json(&json!({
                "rejected": false,
                "temp_path": staged.display().to_string(),
                "blake3": accepted.blake3_hex,
                "size": accepted.size,
            })))
        }
        Err(reason) => {
            // Audit on reject. Resolved path may be unknown
            // (path-unresolvable / outside-roots variants both
            // shape it differently); pull what we have.
            let resolved_owned = match &reason {
                RejectReason::OutsideAllowedRoots { resolved } => {
                    resolved.to_str().map(|s| s.to_string())
                }
                _ => None,
            };
            let human = reason.human();
            let entry = AuditEntry {
                ts: now_rfc3339(),
                path: &a.path,
                resolved: resolved_owned.as_deref(),
                outcome: "reject",
                size: None,
                blake3: None,
                reason: Some(human.clone()),
            };
            if let Err(e) = append_audit(audit_path.as_deref(), &entry) {
                tracing::warn!(error = %e, "audit log append failed (reject)");
            }

            // Reject notification: telegram + project-wide Wire
            // (channel:<project>:all). Both rows go through the
            // existing send_dm path so team-bot's outbound loop and
            // the TUI's Wire-tab query both see them without new
            // wiring.
            let notice = format!("📎 broker rejected attachment {}: {human}", a.path);
            let project = ctx.project();
            let broker_id = format!("{project}:broker");
            let _ = ctx
                .store
                .send_dm(project, &broker_id, "user:telegram", &notice, None, None);
            let wire_recipient = format!("channel:{project}:all");
            let _ = ctx
                .store
                .send_dm(project, &broker_id, &wire_recipient, &notice, None, None);

            Ok(content_json(&json!({
                "rejected": true,
                "reason": human,
            })))
        }
    }
}

/// Resolve an `audit_log_path` from compose against the compose
/// root: relative paths join, absolute paths pass through.
fn resolve_audit_path(compose_root: &std::path::Path, p: &std::path::Path) -> std::path::PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        compose_root.join(p)
    }
}

/// T-109: deliver `/compact` to the calling agent's own tmux pane via
/// `tmux send-keys`. Open to any agent (workers and managers) on
/// `claude-code` runtimes — the destructive warning lives in the schema
/// description, role instructions decide when to call it. Fire-and-forget:
/// the MCP call returns immediately; the slash command lands in the
/// agent's pane on the blocking pool. Detachable — when Anthropic ships
/// native agent-driven compaction, this whole handler + its registry
/// entries delete in one PR.
async fn compact_self(ctx: &Ctx) -> Result<Value, String> {
    let runtime = ctx
        .store
        .runtime_for(&ctx.agent_id)
        .map_err(|e| e.to_string())?;
    if runtime.as_deref() != Some("claude-code") {
        return Err(format!(
            "compact_self: /compact is only supported on Claude Code agents \
             (caller={} runs `{}`)",
            ctx.agent_id,
            runtime.as_deref().unwrap_or("unknown"),
        ));
    }
    let session = pane_session(&ctx.tmux_prefix, &ctx.agent_id)?;
    // Fire-and-forget. Run the blocking tmux invoke on the blocking pool
    // and drop the join handle so the MCP response goes back without
    // waiting for tmux. On failure we log and drop — the caller deliberately
    // doesn't observe send-keys errors (that's the point of fire-and-forget).
    let session_for_spawn = session.clone();
    tokio::task::spawn_blocking(move || {
        let argv = compact_self_argv(&session_for_spawn);
        match std::process::Command::new("tmux").args(argv).output() {
            Ok(o) if !o.status.success() => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let trimmed = stderr.trim();
                tracing::warn!(
                    session = %session_for_spawn,
                    error = %trimmed,
                    "compact_self: tmux send-keys failed",
                );
            }
            Err(e) => {
                tracing::warn!(
                    session = %session_for_spawn,
                    error = %e,
                    "compact_self: tmux invoke failed",
                );
            }
            _ => {}
        }
    });
    Ok(content_json(&json!({
        "status": "dispatched",
        "session": session,
    })))
}

/// Argv for the tmux send-keys invocation that delivers `/compact` to
/// the caller's pane. Pulled out so unit tests pin the exact arg shape
/// without spinning up tmux. The trailing `Enter` keyword is what tells
/// tmux to fire a Return after the body — that's what triggers Claude
/// Code to actually process the slash command.
fn compact_self_argv(session: &str) -> [&str; 5] {
    ["send-keys", "-t", session, "/compact", "Enter"]
}

/// Compose the tmux session name for `agent_id` under `tmux_prefix`,
/// matching the supervisor's canonical formula
/// (`{tmux_prefix}{project}-{role}`).
fn pane_session(tmux_prefix: &str, agent_id: &str) -> Result<String, String> {
    let (project, role) = agent_id.split_once(':').ok_or_else(|| {
        format!("compact_self: malformed agent id `{agent_id}` (expected `project:role`)")
    })?;
    Ok(format!("{tmux_prefix}{project}-{role}"))
}

#[derive(Deserialize)]
struct BroadcastArgs {
    channel: String,
    text: String,
}

fn broadcast(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: BroadcastArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let project = ctx.project();
    if !ctx
        .store
        .is_channel_member(project, &a.channel, &ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {agent} is not a member of channel {channel} in project {project}",
            agent = ctx.agent_id,
            channel = a.channel,
        ));
    }
    if !ctx
        .store
        .can_broadcast(&ctx.agent_id, &a.channel)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {agent} is not permitted to broadcast on {channel}",
            agent = ctx.agent_id,
            channel = a.channel,
        ));
    }
    let id = ctx
        .store
        .send_broadcast(project, &ctx.agent_id, &a.channel, &a.text)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "channel": a.channel })))
}

#[derive(Deserialize, Default)]
struct InboxPeekArgs {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    20
}

fn inbox_peek(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxPeekArgs = if args.is_null() {
        InboxPeekArgs::default()
    } else {
        serde_json::from_value(args).map_err(|e| e.to_string())?
    };
    let msgs = ctx
        .store
        .inbox_peek(&ctx.agent_id, a.limit)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "messages": msgs })))
}

#[derive(Deserialize)]
struct InboxAckArgs {
    ids: Vec<i64>,
}

fn inbox_ack(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxAckArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let n = ctx.store.inbox_ack(&a.ids).map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "acked": n })))
}

#[derive(Deserialize)]
struct InboxReadArgs {
    ids: Vec<i64>,
}

fn inbox_read(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxReadArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let msgs = ctx
        .store
        .inbox_read(&ctx.agent_id, &a.ids)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "messages": msgs })))
}

#[derive(Deserialize, Default)]
struct InboxWatchArgs {
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
fn default_timeout() -> u64 {
    15000
}

async fn inbox_watch(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxWatchArgs = if args.is_null() {
        InboxWatchArgs::default()
    } else {
        serde_json::from_value(args).map_err(|e| e.to_string())?
    };
    // Poll every 250 ms up to the deadline.
    let mut remaining = a.timeout_ms;
    loop {
        let msgs = ctx
            .store
            .inbox_peek(&ctx.agent_id, 20)
            .map_err(|e| e.to_string())?;
        if !msgs.is_empty() || remaining == 0 {
            return Ok(content_json(&json!({ "messages": msgs })));
        }
        let step = remaining.min(250);
        sleep(Duration::from_millis(step)).await;
        remaining -= step;
    }
}

fn list_team(ctx: &Ctx) -> Result<Value, String> {
    let ids = ctx
        .store
        .list_project_agents(ctx.project())
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "agents": ids })))
}

fn org_chart(ctx: &Ctx) -> Result<Value, String> {
    let v = ctx
        .store
        .org_chart(ctx.project())
        .map_err(|e| e.to_string())?;
    Ok(content_json(&v))
}

#[derive(Deserialize)]
struct ApprovalArgs {
    action: String,
    #[serde(default)]
    scope_tag: Option<String>,
    summary: String,
    #[serde(default)]
    payload: Value,
    #[serde(default = "default_approval_ttl")]
    ttl_seconds: u64,
    #[serde(default = "default_approval_wait")]
    wait: bool,
}
fn default_approval_ttl() -> u64 {
    900
}
fn default_approval_wait() -> bool {
    true
}

async fn request_approval(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ApprovalArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let payload_str = serde_json::to_string(&a.payload).unwrap_or_else(|_| "{}".into());
    let id = ctx
        .store
        .request_approval(
            ctx.project(),
            &ctx.agent_id,
            &a.action,
            a.scope_tag.as_deref(),
            &a.summary,
            &payload_str,
            a.ttl_seconds as f64,
        )
        .map_err(|e| e.to_string())?;

    if !a.wait {
        let (status, note, delivered_at) =
            ctx.store.approval_status(id).map_err(|e| e.to_string())?;
        return Ok(content_json(&json!({
            "id": id,
            "status": status,
            "note": note,
            "delivered_at": delivered_at,
        })));
    }

    // Poll every 500 ms until decided or expired.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(a.ttl_seconds);
    loop {
        let _ = ctx.store.expire_stale_approvals();
        let (status, note, delivered_at) =
            ctx.store.approval_status(id).map_err(|e| e.to_string())?;
        if status != "pending" {
            return Ok(content_json(&json!({
                "id": id,
                "status": status,
                "note": note,
                "delivered_at": delivered_at,
            })));
        }
        if std::time::Instant::now() >= deadline {
            // Force-expire one last time.
            let _ = ctx.store.expire_stale_approvals();
            let (status, note, delivered_at) =
                ctx.store.approval_status(id).map_err(|e| e.to_string())?;
            return Ok(content_json(&json!({
                "id": id,
                "status": status,
                "note": note,
                "delivered_at": delivered_at,
            })));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::NamedTempFile;

    fn ctx_with_manager() -> (Ctx, NamedTempFile) {
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:mgr", "p", "P", "manager", "claude-code", true)
            .unwrap();
        (Ctx::new("p:mgr".to_string(), store, "t-".to_string()), f)
    }

    fn fetch_message(store: &Store, id: i64) -> (String, Option<String>, Option<String>) {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT text, kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn reply_to_user_text_only_back_compat() {
        // Existing text-only callers see the legacy code path: kind +
        // structured_payload stay NULL, response carries `id`. Pins R5
        // back-compat from the umbrella's acceptance criteria.
        let (ctx, _f) = ctx_with_manager();
        let resp = reply_to_user(&ctx, json!({ "text": "hello" }))
            .await
            .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        let (text, kind, payload) = fetch_message(&ctx.store, id);
        assert_eq!(text, "hello");
        assert!(kind.is_none(), "text-only must leave kind NULL");
        assert!(payload.is_none());
    }

    #[tokio::test]
    async fn reply_to_user_image_url_inserts_structured_row() {
        // URL source bypasses local validation — we trust the caller and
        // let Telegram validate server-side. Pins the kind/payload columns.
        let (ctx, _f) = ctx_with_manager();
        let resp = reply_to_user(
            &ctx,
            json!({
                "image": {
                    "source": "url",
                    "value": "https://example.com/a.png",
                    "caption": "PR ready"
                }
            }),
        )
        .await
        .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        let (text, kind, payload) = fetch_message(&ctx.store, id);
        assert_eq!(text, "PR ready", "caption mirrors into text column");
        assert_eq!(kind.as_deref(), Some("image"));
        let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
        assert_eq!(p["source"], "url");
        assert_eq!(p["value"], "https://example.com/a.png");
        assert_eq!(p["caption"], "PR ready");
    }

    #[tokio::test]
    async fn reply_to_user_image_path_round_trip() {
        // path source: real file on disk under the size cap and within
        // the mime allowlist. Pins the kind=image row + the structured
        // payload string the bot's outbound dispatcher will parse.
        let (ctx, _f) = ctx_with_manager();
        let img = NamedTempFile::with_suffix(".png").unwrap();
        std::fs::write(img.path(), b"not really a png").unwrap();
        let resp = reply_to_user(
            &ctx,
            json!({
                "image": {
                    "source": "path",
                    "value": img.path().to_str().unwrap(),
                    "caption": "screenshot"
                }
            }),
        )
        .await
        .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        let (_, kind, payload) = fetch_message(&ctx.store, id);
        assert_eq!(kind.as_deref(), Some("image"));
        let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
        assert_eq!(p["source"], "path");
        assert_eq!(p["value"], img.path().to_str().unwrap());
    }

    #[tokio::test]
    async fn reply_to_user_text_plus_image_inserts_two_rows() {
        // Multi-content shape: one tool call → one text row + one image
        // row, returned as the `ids` array. Order is text-first, image-
        // next so the operator reads the framing line before the photo.
        let (ctx, _f) = ctx_with_manager();
        let resp = reply_to_user(
            &ctx,
            json!({
                "text": "here's the latest design",
                "image": { "source": "url", "value": "https://example.com/d.png" }
            }),
        )
        .await
        .unwrap();
        let ids: Vec<i64> = resp["structuredContent"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        let (text0, kind0, _) = fetch_message(&ctx.store, ids[0]);
        let (_, kind1, _) = fetch_message(&ctx.store, ids[1]);
        assert_eq!(text0, "here's the latest design");
        assert!(kind0.is_none(), "first row is text");
        assert_eq!(kind1.as_deref(), Some("image"));
    }

    #[tokio::test]
    async fn reply_to_user_rejects_disallowed_image_extension() {
        let (ctx, _f) = ctx_with_manager();
        let f = NamedTempFile::with_suffix(".bmp").unwrap();
        std::fs::write(f.path(), b"x").unwrap();
        let err = reply_to_user(
            &ctx,
            json!({ "image": { "source": "path", "value": f.path().to_str().unwrap() } }),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("allowlist"),
            "error must name the mime allowlist: {err}"
        );
    }

    #[tokio::test]
    async fn reply_to_user_rejects_oversize_path() {
        let (ctx, _f) = ctx_with_manager();
        let big = NamedTempFile::with_suffix(".png").unwrap();
        // Sparse-write a file 1 byte past the 50MB cap — fast and doesn't
        // need the bytes to be real.
        big.as_file()
            .set_len(MEDIA_MAX_BYTES + 1)
            .expect("sparse extend");
        let err = reply_to_user(
            &ctx,
            json!({ "image": { "source": "path", "value": big.path().to_str().unwrap() } }),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("50MB"),
            "error must reference the size cap: {err}"
        );
    }

    #[tokio::test]
    async fn reply_to_user_rejects_missing_path() {
        let (ctx, _f) = ctx_with_manager();
        let err = reply_to_user(
            &ctx,
            json!({
                "image": { "source": "path", "value": "/nonexistent/thing.png" }
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("not readable"),
            "error must name the unreadable path: {err}"
        );
    }

    #[tokio::test]
    async fn reply_to_user_rejects_empty_call() {
        let (ctx, _f) = ctx_with_manager();
        let err = reply_to_user(&ctx, json!({})).await.unwrap_err();
        assert!(
            err.contains("at least one"),
            "empty call must surface the at-least-one constraint: {err}"
        );
    }

    #[tokio::test]
    async fn reply_to_user_non_manager_is_rejected_before_validation() {
        // R8 manager-gating: a worker call hits the is_manager check
        // first, so we never even read the disk for path-source media.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
        let err = reply_to_user(
            &ctx,
            json!({
                "image": { "source": "path", "value": "/nonexistent/x.png" }
            }),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("only managers"),
            "non-manager must be gated before media validation: {err}"
        );
    }

    #[test]
    fn image_extension_allowlist_accepts_canonical_set_and_rejects_others() {
        for ok in [
            "/tmp/a.jpg",
            "/tmp/a.JPEG",
            "/tmp/photo.PNG",
            "/tmp/sticker.webp",
            "/tmp/loop.gif",
        ] {
            assert!(image_extension_allowed(ok), "should accept: {ok}");
        }
        for bad in ["/tmp/a.bmp", "/tmp/a.tiff", "/tmp/a.svg", "/tmp/no_ext"] {
            assert!(!image_extension_allowed(bad), "should reject: {bad}");
        }
    }

    // ── T-086-E react_to_user ──────────────────────────────────────

    fn fetch_kind_and_payload(store: &Store, id: i64) -> (Option<String>, Option<String>) {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn react_to_user_persists_kind_and_structured_payload() {
        // Affirmative path: agent calls with a supported emoji + valid
        // message id; row lands with `kind = "reaction"` and a payload
        // carrying both fields the bot dispatcher needs.
        let (ctx, _f) = ctx_with_manager();
        let resp = react_to_user(&ctx, json!({ "telegram_msg_id": 4242, "emoji": "👀" }))
            .await
            .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        let (kind, payload) = fetch_kind_and_payload(&ctx.store, id);
        assert_eq!(kind.as_deref(), Some("reaction"));
        let p: Value = serde_json::from_str(&payload.unwrap()).unwrap();
        assert_eq!(p["telegram_msg_id"], 4242);
        assert_eq!(p["emoji"], "👀");
    }

    #[tokio::test]
    async fn react_to_user_returns_message_id_and_emoji_in_response() {
        // The structured response surfaces both the new mailbox row id
        // and the (telegram_msg_id, emoji) pair so callers can correlate
        // their request with the eventual outbound API call.
        let (ctx, _f) = ctx_with_manager();
        let resp = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🎉" }))
            .await
            .unwrap();
        assert_eq!(resp["structuredContent"]["telegram_msg_id"], 7);
        assert_eq!(resp["structuredContent"]["emoji"], "🎉");
        assert_eq!(resp["structuredContent"]["recipient"], "user:telegram");
    }

    #[tokio::test]
    async fn react_to_user_rejects_out_of_allowlist_emoji() {
        // Defence in depth: out-of-set emoji surfaces a clean MCP error
        // rather than reaching Telegram and getting a server-side
        // rejection that would land in the bot's tracing::warn! log
        // instead of the agent's tool-call response.
        let (ctx, _f) = ctx_with_manager();
        let err = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🍕" }))
            .await
            .unwrap_err();
        assert!(err.contains("allowlist"), "error names the gate: {err}");
        assert!(
            err.contains("🍕"),
            "error includes the rejected emoji: {err}"
        );
    }

    #[tokio::test]
    async fn react_to_user_non_manager_is_rejected_before_validation() {
        // R8 manager-gating: a worker call hits the is_manager check
        // first, so we never reach allowlist validation. Mirrors the
        // PR #81 (T-086-A) reply_to_user gating shape.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
        let err = react_to_user(&ctx, json!({ "telegram_msg_id": 7, "emoji": "🍕" }))
            .await
            .unwrap_err();
        assert!(
            err.contains("only managers"),
            "non-manager must be gated before allowlist check: {err}"
        );
    }

    #[tokio::test]
    async fn react_to_user_rejects_missing_telegram_msg_id() {
        // Schema's `required: ["telegram_msg_id", "emoji"]` is
        // enforced at deserialization time; pinning the rejection so a
        // future schema-shape regression surfaces here.
        let (ctx, _f) = ctx_with_manager();
        let err = react_to_user(&ctx, json!({ "emoji": "👍" }))
            .await
            .unwrap_err();
        assert!(
            err.contains("telegram_msg_id") || err.contains("missing"),
            "missing required field error: {err}"
        );
    }

    // ── T-102 show_typing ──────────────────────────────────────────

    #[tokio::test]
    async fn show_typing_persists_kind_and_empty_payload() {
        // Affirmative path: a manager-side call lands a row keyed by
        // `kind = "typing"` with an empty JSON payload. The bot's
        // dispatcher discriminates on `kind` alone, so the payload only
        // has to be valid JSON.
        let (ctx, _f) = ctx_with_manager();
        let resp = show_typing(&ctx).await.unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        let (kind, payload) = fetch_kind_and_payload(&ctx.store, id);
        assert_eq!(kind.as_deref(), Some("typing"));
        assert_eq!(payload.as_deref(), Some("{}"));
    }

    #[tokio::test]
    async fn show_typing_returns_message_id_and_recipient() {
        // Response shape: callers get the new mailbox row id back so
        // they can correlate it (mostly useful in tests + diagnostics —
        // production agents fire-and-forget) and the recipient string
        // matches the same constant `reply_to_user` and `react_to_user`
        // emit, so a single response shape spans the manager-side
        // tools.
        let (ctx, _f) = ctx_with_manager();
        let resp = show_typing(&ctx).await.unwrap();
        assert!(resp["structuredContent"]["id"].as_i64().is_some());
        assert_eq!(resp["structuredContent"]["recipient"], "user:telegram");
    }

    #[tokio::test]
    async fn show_typing_non_manager_is_rejected() {
        // Manager gate: workers can't open a typing window. Mirrors the
        // `reply_to_user` / `react_to_user` gating so the manager-only
        // surface stays consistent.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
        let err = show_typing(&ctx).await.unwrap_err();
        assert!(
            err.contains("only managers"),
            "non-manager must be gated: {err}"
        );
    }

    // ── T-109 compact_self ─────────────────────────────────────────

    #[test]
    fn compact_self_argv_uses_enter_keyword_after_compact_body() {
        // Pin the wire shape: tmux receives `Enter` as a separate argv
        // element, NOT `\n` embedded in the body. That's what makes tmux
        // fire a Return after `/compact`, which is what makes Claude Code
        // actually process the slash command.
        let argv = compact_self_argv("t-p-mgr");
        assert_eq!(argv, ["send-keys", "-t", "t-p-mgr", "/compact", "Enter"]);
    }

    #[test]
    fn pane_session_matches_supervisor_canonical_formula() {
        // Same shape as `team-core::supervisor::AgentSpec::from_handle`
        // and `team-bot::slash_outcome` — keep the three call sites in
        // sync. If this assert ever has to change, all three move
        // together.
        assert_eq!(pane_session("t-", "p:mgr").unwrap(), "t-p-mgr");
        assert_eq!(pane_session("", "p:mgr").unwrap(), "p-mgr");
        assert_eq!(
            pane_session("teamctl-", "alpha:hugo").unwrap(),
            "teamctl-alpha-hugo"
        );
    }

    #[test]
    fn pane_session_rejects_malformed_agent_id() {
        // The `<project>:<agent>` invariant is upheld at the MCP boundary
        // so the handler doesn't fire tmux against a garbage session
        // name. Surface the malformed id in the error so an operator
        // can trace which client misconfigured itself.
        let err = pane_session("t-", "no-colon-here").unwrap_err();
        assert!(err.contains("malformed"), "error names the gate: {err}");
        assert!(err.contains("no-colon-here"), "error names the id: {err}");
    }

    #[tokio::test]
    async fn compact_self_dispatches_for_claude_code_manager() {
        // Happy path: a claude-code manager call returns the dispatch
        // record with the resolved session name. The actual tmux side
        // effect runs on the blocking pool and is not observed here —
        // fire-and-forget is the contract.
        let (ctx, _f) = ctx_with_manager();
        let resp = compact_self(&ctx).await.unwrap();
        assert_eq!(resp["structuredContent"]["status"], "dispatched");
        assert_eq!(resp["structuredContent"]["session"], "t-p-mgr");
    }

    #[tokio::test]
    async fn compact_self_dispatches_for_worker_too() {
        // No manager gate: workers can self-compact when their role
        // instruction says so. The destructive warning in the schema
        // description carries the safety; the tool itself is open to
        // any claude-code agent.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let ctx = Ctx::new("p:dev".to_string(), store, "t-".to_string());
        let resp = compact_self(&ctx).await.unwrap();
        assert_eq!(resp["structuredContent"]["status"], "dispatched");
        assert_eq!(resp["structuredContent"]["session"], "t-p-dev");
    }

    #[tokio::test]
    async fn compact_self_non_claude_code_runtime_is_rejected() {
        // /compact is a Claude Code slash command. Other runtimes don't
        // recognize it; sending it would just land as input. Reject so
        // the failure mode is crisp and the tool's description stays
        // honest.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:mgr", "p", "P", "manager", "codex", true)
            .unwrap();
        let ctx = Ctx::new("p:mgr".to_string(), store, "t-".to_string());
        let err = compact_self(&ctx).await.unwrap_err();
        assert!(
            err.contains("Claude Code"),
            "error names the runtime gate: {err}"
        );
        assert!(
            err.contains("codex"),
            "error names the actual runtime: {err}"
        );
    }

    #[test]
    fn bot_reaction_allowlist_accepts_canonical_set_and_rejects_pizza() {
        // Spot-check both directions on the in-memory allowlist —
        // canonical entries pass; an obvious non-entry fails.
        for ok in ["👍", "👎", "❤️", "🎉", "👀", "🤝", "👨\u{200d}💻"] {
            assert!(is_allowed_reaction(ok), "should accept: {ok}");
        }
        for bad in ["🍕", "🥑", "abc", ""] {
            assert!(!is_allowed_reaction(bad), "should reject: {bad}");
        }
    }

    // ── T-168 reply_to_message_id (mailbox-id semantics) ───────────

    fn fetch_telegram_msg_id(store: &Store, id: i64) -> Option<i64> {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT telegram_msg_id FROM messages WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Hand-insert an inbound `user:telegram` row with a known
    /// `telegram_msg_id` and return its mailbox id — what an agent
    /// would see as `meta.id` in the channel envelope.
    fn seed_inbound_row(store: &Store, telegram_msg_id: i64) -> i64 {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, telegram_msg_id)
             VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, ?1)",
            params![telegram_msg_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[tokio::test]
    async fn reply_to_user_threads_text_when_reply_to_message_id_set() {
        // T-168 happy path: agent passes the inbound row's mailbox id
        // (`meta.id`); the store resolves it to the row's Telegram id
        // at insert and persists THAT on the outbound row so the bot's
        // dispatcher can hand it to `reply_parameters`.
        let (ctx, _f) = ctx_with_manager();
        let inbound = seed_inbound_row(&ctx.store, 12345);
        let resp = reply_to_user(
            &ctx,
            json!({ "text": "ack", "reply_to_message_id": inbound }),
        )
        .await
        .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        assert_eq!(fetch_telegram_msg_id(&ctx.store, id), Some(12345));
    }

    #[tokio::test]
    async fn reply_to_user_text_only_back_compat_leaves_telegram_msg_id_null() {
        // R5 back-compat: existing callers that don't pass
        // `reply_to_message_id` see the same NULL column they always
        // did. Bot dispatcher reads NULL → omits `reply_parameters` →
        // message lands as a fresh post.
        let (ctx, _f) = ctx_with_manager();
        let resp = reply_to_user(&ctx, json!({ "text": "ack" })).await.unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        assert!(fetch_telegram_msg_id(&ctx.store, id).is_none());
    }

    #[tokio::test]
    async fn reply_to_user_threads_image_when_reply_to_message_id_set() {
        // Threading + media: image attached as a reply nests under
        // the parent message in Telegram. The outbound media row
        // carries the resolved `telegram_msg_id` the text path does.
        let (ctx, _f) = ctx_with_manager();
        let inbound = seed_inbound_row(&ctx.store, 7);
        let resp = reply_to_user(
            &ctx,
            json!({
                "image": {
                    "source": "url",
                    "value": "https://example.com/a.png",
                    "caption": "screenshot"
                },
                "reply_to_message_id": inbound
            }),
        )
        .await
        .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        assert_eq!(fetch_telegram_msg_id(&ctx.store, id), Some(7));
    }

    #[tokio::test]
    async fn reply_to_user_text_plus_image_share_one_reply_to_message_id() {
        // Multi-content shape: one tool call → two outbound rows
        // (text + image) — both resolve from the same inbound mailbox
        // id, so the operator sees both replies threaded under the
        // same parent in Telegram, not split into two separate threads.
        let (ctx, _f) = ctx_with_manager();
        let inbound = seed_inbound_row(&ctx.store, 99);
        let resp = reply_to_user(
            &ctx,
            json!({
                "text": "fixing",
                "image": { "source": "url", "value": "https://example.com/d.png" },
                "reply_to_message_id": inbound
            }),
        )
        .await
        .unwrap();
        let ids: Vec<i64> = resp["structuredContent"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        for id in ids {
            assert_eq!(
                fetch_telegram_msg_id(&ctx.store, id),
                Some(99),
                "every row in the call shares the same reply target"
            );
        }
    }

    #[tokio::test]
    async fn reply_to_user_resolution_misses_leave_telegram_msg_id_null() {
        // T-168 miss path: agent references a mailbox id that doesn't
        // resolve (stale id, or row without telegram_msg_id). The
        // outbound row sends without threading rather than failing
        // the call — observability comes from the warn-log in
        // resolve_telegram_msg_id, not a tool-level error.
        let (ctx, _f) = ctx_with_manager();
        let resp = reply_to_user(
            &ctx,
            json!({ "text": "ack", "reply_to_message_id": 999_999 }),
        )
        .await
        .unwrap();
        let id = resp["structuredContent"]["id"].as_i64().unwrap();
        assert!(fetch_telegram_msg_id(&ctx.store, id).is_none());
    }

    // ── T-32b read_attachment ──────────────────────────────────────

    fn ctx_with_attachments(
        cfg: team_core::compose::Attachments,
    ) -> (Ctx, NamedTempFile, tempfile::TempDir) {
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let compose_root = tempfile::tempdir().unwrap();
        let ctx = Ctx::for_test_with_attachments(
            "p:dev".to_string(),
            store,
            compose_root.path(),
            cfg,
            None,
        );
        (ctx, f, compose_root)
    }

    fn cfg_with_root_only(root: &std::path::Path) -> team_core::compose::Attachments {
        team_core::compose::Attachments {
            enabled: true,
            max_size_bytes: 1024 * 1024,
            allowed_roots: vec![root.to_string_lossy().into_owned()],
            scanner: None,
            audit_log_path: None,
            tempfile_ttl_seconds: 6 * 60 * 60,
        }
    }

    #[tokio::test]
    async fn read_attachment_no_compose_returns_disabled() {
        // Hand-launched / no-compose-root case: tool returns the
        // disabled-style envelope rather than touching the
        // filesystem. Pinning so a refactor that removes the early
        // exit doesn't silently start serving raw bytes.
        let f = NamedTempFile::new().unwrap();
        let store = Store::open(f.path()).unwrap();
        store
            .upsert_agent("p:dev", "p", "P", "dev", "claude-code", false)
            .unwrap();
        let ctx = Ctx::new("p:dev".to_string(), store, "t-".into());
        let resp = read_attachment(&ctx, json!({ "path": "/etc/passwd" }))
            .await
            .unwrap();
        assert_eq!(resp["structuredContent"]["rejected"], true);
        let reason = resp["structuredContent"]["reason"].as_str().unwrap();
        assert!(
            reason.contains("attachments unavailable"),
            "reason: {reason}"
        );
    }

    #[tokio::test]
    async fn read_attachment_happy_path_stages_and_returns_path() {
        let work = tempfile::tempdir().unwrap();
        let payload = work.path().join("note.md");
        std::fs::write(&payload, b"the build is green").unwrap();
        let cfg = cfg_with_root_only(work.path());
        let (ctx, _f, compose_root) = ctx_with_attachments(cfg);
        let resp = read_attachment(&ctx, json!({ "path": payload.display().to_string() }))
            .await
            .unwrap();
        assert_eq!(resp["structuredContent"]["rejected"], false);
        let temp_path = resp["structuredContent"]["temp_path"].as_str().unwrap();
        let staged_bytes = std::fs::read(temp_path).unwrap();
        assert_eq!(staged_bytes, b"the build is green");
        // Staged under the compose root's staging dir.
        assert!(
            temp_path.starts_with(
                compose_root
                    .path()
                    .join("state/attachments-staging")
                    .to_str()
                    .unwrap()
            ),
            "staged path under compose root: {temp_path}"
        );
        // Response carries the blake3 + size for the agent's audit
        // hooks (and to detect mid-stream tampering).
        assert!(resp["structuredContent"]["blake3"].is_string());
        assert_eq!(resp["structuredContent"]["size"].as_u64(), Some(18));
    }

    #[tokio::test]
    async fn read_attachment_reject_writes_telegram_and_wire_rows() {
        // Outside-allowed-roots reject: agent gets the rejection
        // envelope, AND two notification rows land in the mailbox —
        // one to user:telegram for the team-bot to forward, one to
        // channel:p:all so the operator's TUI Wire tab surfaces the
        // reason. Owner-ratify variant 3 (option c) requires both.
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let leak = outside.path().join("leak.txt");
        std::fs::write(&leak, b"x").unwrap();
        let cfg = cfg_with_root_only(inside.path());
        let (ctx, _f, _compose_root) = ctx_with_attachments(cfg);
        let resp = read_attachment(&ctx, json!({ "path": leak.display().to_string() }))
            .await
            .unwrap();
        assert_eq!(resp["structuredContent"]["rejected"], true);

        // Inspect mailbox rows: expect exactly two reject rows from
        // the broker (telegram + wire), text containing the path.
        let conn = ctx.store.conn.lock().unwrap();
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT recipient, text FROM messages ORDER BY id ASC")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
        let recipients: Vec<&str> = rows.iter().map(|(r, _)| r.as_str()).collect();
        assert!(
            recipients.contains(&"user:telegram"),
            "telegram reject row: {recipients:?}"
        );
        assert!(
            recipients.contains(&"channel:p:all"),
            "wire reject row: {recipients:?}"
        );
        // Both notifications mention the operator-supplied path.
        for (_, text) in &rows {
            assert!(
                text.contains(&leak.display().to_string()),
                "reject text mentions path: {text}"
            );
        }
    }

    #[tokio::test]
    async fn read_attachment_disabled_returns_reject_without_filesystem_touch() {
        let work = tempfile::tempdir().unwrap();
        let payload = work.path().join("note.md");
        std::fs::write(&payload, b"x").unwrap();
        let mut cfg = cfg_with_root_only(work.path());
        cfg.enabled = false;
        let (ctx, _f, _root) = ctx_with_attachments(cfg);
        let resp = read_attachment(&ctx, json!({ "path": payload.display().to_string() }))
            .await
            .unwrap();
        assert_eq!(resp["structuredContent"]["rejected"], true);
        let reason = resp["structuredContent"]["reason"].as_str().unwrap();
        assert!(reason.contains("disabled"), "reason: {reason}");
    }

    #[tokio::test]
    async fn read_attachment_audit_log_records_accept_and_reject() {
        // Two attempts, one accept + one reject, both lines land
        // in the audit file. Each line is parseable JSON.
        let work = tempfile::tempdir().unwrap();
        let ok_path = work.path().join("ok.md");
        std::fs::write(&ok_path, b"hi").unwrap();
        let bogus = work.path().join("missing.md");
        let mut cfg = cfg_with_root_only(work.path());
        let audit_path = work.path().join("audit/attempts.log");
        cfg.audit_log_path = Some(audit_path.clone());
        let (ctx, _f, _root) = ctx_with_attachments(cfg);
        let _ = read_attachment(&ctx, json!({ "path": ok_path.display().to_string() }))
            .await
            .unwrap();
        let _ = read_attachment(&ctx, json!({ "path": bogus.display().to_string() }))
            .await
            .unwrap();
        let body = std::fs::read_to_string(&audit_path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "two attempts, two lines: {body}");
        let outcomes: Vec<String> = lines
            .iter()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["outcome"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(outcomes, vec!["accept", "reject"]);
    }

    #[tokio::test]
    async fn read_attachment_missing_path_arg_is_rejected_at_schema() {
        // `required: ["path"]` enforced at deserialization; a
        // schema-shape regression would surface as a deserialize
        // error here, not as an MCP-level success.
        let (ctx, _f, _root) =
            ctx_with_attachments(cfg_with_root_only(tempfile::tempdir().unwrap().path()));
        let err = read_attachment(&ctx, json!({})).await.unwrap_err();
        assert!(
            err.contains("path") || err.contains("missing"),
            "missing required field error: {err}"
        );
    }
}
