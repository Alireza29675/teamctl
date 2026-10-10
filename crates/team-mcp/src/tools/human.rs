use serde::Deserialize;
use serde_json::{json, Value};

use super::{content_json, Ctx};

#[derive(Deserialize)]
pub(super) struct ReplyToUserArgs {
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

/// Per-file size cap matching Telegram's bot-API ceiling for photo/document
/// uploads. URLs bypass the local check — Telegram will validate on its end.
pub(super) const MEDIA_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Telegram Bot API caps a single text message at 4096 and a media caption
/// at 1024. The unit is **UTF-16 code units**, not bytes or Unicode
/// scalars — the same unit Telegram uses for entity offsets (a non-BMP
/// emoji counts as 2, most CJK/Latin chars as 1). We reject above these at
/// the MCP boundary so the agent gets an actionable error: the bot's
/// outbound dispatch only `warn`-logs Telegram's rejection (#168), which
/// is invisible to the agent — it already holds an OK from this tool and
/// moves on. Limits per Telegram Bot API as of 2026-05 (`sendMessage`
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
    // Length-gate before any row insert. Telegram silent-drops oversized
    // content and the bot only warn-logs that (#168) — reject here so the
    // agent sees an actionable error instead of a false success. #252.
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

pub(super) fn is_allowed_reaction(emoji: &str) -> bool {
    BOT_REACTION_ALLOWLIST.contains(&emoji)
}

#[derive(Deserialize)]
pub(super) struct ReactToUserArgs {
    telegram_msg_id: i64,
    emoji: String,
}

pub(super) async fn react_to_user(ctx: &Ctx, args: Value) -> Result<Value, String> {
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
pub(super) async fn show_typing(ctx: &Ctx) -> Result<Value, String> {
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
