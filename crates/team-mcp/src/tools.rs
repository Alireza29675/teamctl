//! MCP tool definitions and dispatch.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::store::Store;

pub struct Ctx {
    pub agent_id: String,
    pub store: Arc<Store>,
}

impl Ctx {
    pub fn new(agent_id: String, store: Store) -> Self {
        Self {
            agent_id,
            store: Arc::new(store),
        }
    }

    pub fn project(&self) -> &str {
        self.agent_id.split(':').next().unwrap_or("")
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
            "description": "Return up to `limit` unacked messages addressed to the caller. Non-destructive.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_ack",
            "description": "Mark the listed message ids as acknowledged so they stop appearing in inbox_peek/inbox_watch.",
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
                        "description": "Plain text only — no markdown, no headings, no code fences (none of it renders on chat surfaces like Telegram). Use emojis sparingly to aid scanability (✅ done, ⚠️ caution, 🔧 working, ❓ question). Aim for short, chat-sized messages; split long output into multiple calls rather than sending a wall of text."
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
        "inbox_ack" => inbox_ack(ctx, p.arguments),
        "inbox_watch" => inbox_watch(ctx, p.arguments).await,
        "broadcast" => broadcast(ctx, p.arguments),
        "list_team" => list_team(ctx),
        "org_chart" => org_chart(ctx),
        "request_approval" => request_approval(ctx, p.arguments).await,
        "reply_to_user" => reply_to_user(ctx, p.arguments).await,
        "react_to_user" => react_to_user(ctx, p.arguments).await,
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
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(
        &json!({ "id": id, "recipient": recipient, "telegram_msg_id": a.telegram_msg_id, "emoji": a.emoji }),
    ))
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
        (Ctx::new("p:mgr".to_string(), store), f)
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
        let ctx = Ctx::new("p:dev".to_string(), store);
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
        let ctx = Ctx::new("p:dev".to_string(), store);
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
}
