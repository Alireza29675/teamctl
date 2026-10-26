use serde::Deserialize;
use serde_json::{json, Value};

use super::{content_json, Ctx};
use crate::store::chat::{ControlMessage, ControlResult};

#[derive(Deserialize)]
pub(super) struct ReplyToUserArgs {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    image: Option<MediaArg>,
    #[serde(default)]
    file: Option<MediaArg>,
    #[serde(default)]
    thread_id: Option<String>,
    /// Inbound mailbox `meta.id` pins the human conversation and quoted
    /// provider message for every part of this reply. Legacy Telegram rows
    /// resolve their stored Telegram ID without acquiring a modern route.
    #[serde(default)]
    reply_to_message_id: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct MediaArg {
    source: MediaSource,
    value: String,
    #[serde(default)]
    caption: Option<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum MediaSource {
    Path,
    Url,
}

/// Shared local-media cap preserving the existing Telegram upload policy.
/// URL sources are passed to the destination adapter without fetching here.
pub(super) const MEDIA_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Telegram Bot API caps a single text message at 4096 and a media caption
/// at 1024. The unit is **UTF-16 code units**, not bytes or Unicode
/// scalars — the same unit Telegram uses for entity offsets (a non-BMP
/// emoji counts as 2, most CJK/Latin chars as 1). Rejecting these at the
/// MCP boundary lets the agent correct its content before enqueueing.
/// Limits per Telegram Bot API as of 2026-05 (`sendMessage`
/// text, `sendPhoto`/`sendDocument` caption). Bump here if Telegram
/// changes them. #252.
pub(super) const MAX_TELEGRAM_TEXT_LEN: usize = 4096;
pub(super) const MAX_TELEGRAM_CAPTION_LEN: usize = 1024;

/// Reject `field` when it exceeds `max` Telegram-counted units. Length is
/// measured in UTF-16 code units to match exactly what Telegram counts —
/// byte length would false-reject a legitimate emoji/CJK-heavy message,
/// char count would admit a slightly-oversized one. The error names the
/// field, the limit, the actual size, and the remediation (chunk into
/// independent calls) so the agent can self-correct without an operator
/// in the loop.
pub(super) fn check_telegram_len(field: &str, s: &str, max: usize) -> Result<(), String> {
    let n = s.encode_utf16().count();
    if n > max {
        return Err(format!(
            "reply_to_user: {field} exceeds Telegram's {max}-character limit (got {n}). \
             Split into multiple reply_to_user calls or attach the content as a file — each \
             call sends an independent chat message, so chunking is straightforward."
        ));
    }
    Ok(())
}

/// Image extensions Telegram's `sendPhoto` reliably renders. We accept the
/// caller's claim by extension; sniffing magic bytes would be more rigorous
/// but the failure mode (Telegram rejects misnamed file) surfaces a
/// recoverable error rather than data loss, so the cheap check earns its
/// place over the expensive one.
pub(super) fn image_extension_allowed(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Validate a `path`-source media descriptor: file must exist, be ≤50MB,
/// and (for images) carry an allowlisted extension. URL-source descriptors
/// skip these checks — neither the size nor the mime is knowable without
/// fetching, and Telegram performs both checks server-side anyway.
pub(super) fn validate_media(kind: &str, m: &MediaArg) -> Result<(), String> {
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

pub(super) fn payload_json(m: &MediaArg) -> String {
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

pub(super) async fn reply_to_user(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ReplyToUserArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let text_present = a.text.as_deref().is_some_and(|t| !t.is_empty());
    if !text_present && a.image.is_none() && a.file.is_none() {
        return Err("reply_to_user: at least one of `text`, `image`, `file` must be set".into());
    }
    let mut parts = Vec::new();
    if text_present {
        parts.push(crate::store::chat::ReplyPart {
            text: a.text.clone().unwrap_or_default(),
            kind: "text",
            payload: None,
        });
    }
    for (kind, media) in [("image", &a.image), ("file", &a.file)] {
        if let Some(media) = media {
            parts.push(crate::store::chat::ReplyPart {
                text: media.caption.clone().unwrap_or_default(),
                kind,
                payload: Some(
                    serde_json::from_str(&payload_json(media)).map_err(|e| e.to_string())?,
                ),
            });
        }
    }
    if let Some(messages) = ctx
        .store
        .reply_chat_validated(
            &ctx.agent_id,
            a.conversation_id.as_deref(),
            a.reply_to_message_id,
            a.request_id.as_deref(),
            &parts,
            || validate_reply_media(&a.image, &a.file).map_err(anyhow::Error::msg),
        )
        .map_err(|e| e.to_string())?
    {
        let ids: Vec<_> = messages.iter().map(|m| m.id).collect();
        return Ok(content_json(
            &json!({"id":ids.first().copied().unwrap_or(0),"ids":ids,"chat_message_ids":ids,"conversation_id":messages.first().map(|m|&m.conversation_id),"recipient":"human"}),
        ));
    }
    // Legacy, unaddressed Telegram calls retain their manager-only behavior.
    if !ctx
        .store
        .is_manager(&ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "reply_to_user: only managers can use an unaddressed legacy reply (caller={})",
            ctx.agent_id
        ));
    }
    if a.request_id.is_some() {
        return Err("request_id requires a human conversation route; provide conversation_id or an addressed inbound message".into());
    }
    validate_reply_media(&a.image, &a.file)?;
    // Reject oversized legacy content before any row insert, so the agent
    // can correct a multipart request without leaving a partial delivery.
    if let Some(t) = a.text.as_deref() {
        check_telegram_len("text", t, MAX_TELEGRAM_TEXT_LEN)?;
    }
    if let Some(c) = a.image.as_ref().and_then(|m| m.caption.as_deref()) {
        check_telegram_len("image.caption", c, MAX_TELEGRAM_CAPTION_LEN)?;
    }
    if let Some(c) = a.file.as_ref().and_then(|m| m.caption.as_deref()) {
        check_telegram_len("file.caption", c, MAX_TELEGRAM_CAPTION_LEN)?;
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

fn validate_reply_media(image: &Option<MediaArg>, file: &Option<MediaArg>) -> Result<(), String> {
    for (kind, media) in [("image", image), ("file", file)] {
        if let Some(media) = media {
            validate_media(kind, media)?;
        }
    }
    Ok(())
}

/// Allowed bot-reaction emoji per Telegram's free-tier `setMessageReaction`
/// allowlist (PHASE-1 §3.5). Premium-tier bots get a wider set; we don't
/// assume premium so we mirror Telegram's free-tier allowlist verbatim.
/// Out-of-set emoji are rejected at the MCP boundary so the agent sees a
/// clean error rather than a Telegram API rejection three layers down. The
/// allowlist constant is the single source of truth — if Telegram extends
/// the set in a future bot-API release, refresh here.
pub(super) const BOT_REACTION_ALLOWLIST: &[&str] = &[
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

pub(crate) fn is_allowed_reaction(emoji: &str) -> bool {
    BOT_REACTION_ALLOWLIST.contains(&emoji)
}

#[derive(Deserialize)]
pub(super) struct ReactToUserArgs {
    #[serde(default)]
    telegram_msg_id: Option<i64>,
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    conversation_id: Option<String>,
    emoji: String,
}

pub(super) async fn react_to_user(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ReactToUserArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let telegram_msg_id = match ctx
        .store
        .chat_control(
            &ctx.agent_id,
            a.conversation_id.as_deref(),
            a.message_id,
            ControlMessage::Reaction {
                emoji: &a.emoji,
                telegram_message_id: a.telegram_msg_id,
            },
        )
        .map_err(|e| e.to_string())?
    {
        ControlResult::Modern(result) => return Ok(content_json(&result)),
        ControlResult::Legacy {
            telegram_message_id,
        } => telegram_message_id
            .ok_or("legacy reaction needs telegram_msg_id; new conversations use message_id")?,
    };
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
        "telegram_msg_id": telegram_msg_id,
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
        &json!({ "id": id, "recipient": recipient, "telegram_msg_id": telegram_msg_id, "emoji": a.emoji }),
    ))
}

/// Open a bounded typing window in an authorized human conversation.
/// Adapters consume the control and expiry from the shared delivery queue;
/// legacy Telegram calls retain their existing mailbox discriminator.
pub(super) async fn show_typing(ctx: &Ctx, args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Address {
        #[serde(default)]
        conversation_id: Option<String>,
        #[serde(default)]
        reply_to_message_id: Option<i64>,
    }
    let address: Address = serde_json::from_value(if args.is_null() { json!({}) } else { args })
        .map_err(|e| e.to_string())?;
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        + 10.0;
    if let ControlResult::Modern(result) = ctx
        .store
        .chat_control(
            &ctx.agent_id,
            address.conversation_id.as_deref(),
            address.reply_to_message_id,
            ControlMessage::Typing { expires_at },
        )
        .map_err(|e| e.to_string())?
    {
        return Ok(content_json(&result));
    }
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
            address.reply_to_message_id,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "recipient": recipient })))
}
