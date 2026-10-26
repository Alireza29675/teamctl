use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ParseMode, ReactionType, ReplyParameters};

use crate::render::{html_escape_str, markdown_escape_str, render_html};

/// One mailbox row in the shape the outbound loop forwards. `kind` is `None`
/// for legacy text rows; structured kinds (image, file) carry the JSON
/// payload describing source + value + optional caption. `telegram_msg_id`
/// (T-086-B) is the Telegram message id this row should reply to — when
/// `Some`, the dispatcher attaches `reply_parameters` so the outbound
/// message visually nests under the operator's earlier message.
#[derive(Debug, Clone)]
pub(super) struct MailboxRow {
    pub(super) id: i64,
    pub(super) sender: String,
    pub(super) text: String,
    pub(super) kind: Option<String>,
    pub(super) payload: Option<String>,
    pub(super) telegram_msg_id: Option<i64>,
}

impl MailboxRow {
    pub(super) fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            sender: r.get(1)?,
            text: r.get(2)?,
            kind: r.get(3)?,
            payload: r.get(4)?,
            telegram_msg_id: r.get(5)?,
        })
    }
}

/// Build a teloxide `ReplyParameters` from a stored Telegram message id, or
/// `None` when no threading is requested. Pulled out so unit tests pin the
/// presence/absence call without spinning up a real `Bot` — the `i32` cast
/// is safe because Telegram message ids stay within `i32` range.
pub(super) fn reply_parameters_for(telegram_msg_id: Option<i64>) -> Option<ReplyParameters> {
    telegram_msg_id.map(|id| ReplyParameters::new(MessageId(id as i32)))
}

/// Parsed structured payload — `source` ("path"|"url"), `value` (the path or
/// URL), optional caption. `parse_payload` turns the JSON string into this
/// shape; failure cases fall back to text rendering with the raw payload
/// surfaced so the operator still sees something.
pub(super) struct MediaPayload {
    pub(super) source: String,
    pub(super) value: String,
    pub(super) caption: Option<String>,
}

pub(super) fn parse_payload(payload: &str) -> Option<MediaPayload> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let source = v.get("source")?.as_str()?.to_string();
    let value = v.get("value")?.as_str()?.to_string();
    let caption = v
        .get("caption")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    Some(MediaPayload {
        source,
        value,
        caption,
    })
}

/// Build a teloxide `InputFile` from a parsed payload's source + value.
/// `path` resolves to a local file; `url` parses the value as a URL the
/// Telegram servers fetch directly.
pub(super) fn input_file_from(payload: &MediaPayload) -> Option<InputFile> {
    match payload.source.as_str() {
        "path" => Some(InputFile::file(&payload.value)),
        "url" => Some(InputFile::url(payload.value.parse().ok()?)),
        _ => None,
    }
}

/// Decision the dispatcher makes for a row's `kind`. Kept as a plain enum so
/// it's testable without instantiating a teloxide `Bot`; the actual API call
/// happens in `forward_row` once the decision is made.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DispatchKind {
    Text,
    Image,
    File,
    /// T-086-E: outbound reaction. Payload carries `{telegram_msg_id, emoji}`;
    /// the dispatcher routes through `setMessageReaction` rather than
    /// sending a chat message.
    Reaction,
    /// T-102: outbound typing indicator. No payload fields used — the
    /// row is purely a discriminator that tells the bot to open or
    /// extend a per-chat typing window. The dispatcher fires one
    /// `sendChatAction("typing")` immediately and leaves the periodic
    /// refresh to `typing_refresh_loop`.
    Typing,
    /// Structured row whose payload didn't parse — surface as a text
    /// fallback so the operator sees the raw payload rather than nothing.
    UnknownFallback,
}

pub(super) fn classify_kind(kind: Option<&str>) -> DispatchKind {
    match kind {
        None | Some("text") | Some("") => DispatchKind::Text,
        Some("image") => DispatchKind::Image,
        Some("file") => DispatchKind::File,
        Some("reaction") => DispatchKind::Reaction,
        Some("typing") => DispatchKind::Typing,
        _ => DispatchKind::UnknownFallback,
    }
}

/// T-102 pure helper: open or extend a typing window for `chat`. The
/// new deadline is `now + ceiling`; an existing entry's deadline is
/// overwritten (this is the spec's "second call resets the 10s clock").
/// Returns the freshly written deadline so callers / tests can assert
/// against it.
pub(super) fn extend_typing_window(
    map: &mut HashMap<ChatId, Instant>,
    chat: ChatId,
    now: Instant,
    ceiling: Duration,
) -> Instant {
    let deadline = now + ceiling;
    map.insert(chat, deadline);
    deadline
}

/// T-102 pure helper: clear the typing window for `chat`. Returns
/// whether an entry was actually removed; the dispatcher doesn't act on
/// the bool today, but the return value lets tests pin both the present
/// and absent cases. Called before every text/image/file dispatch so
/// the indicator disappears the moment a real message lands.
pub(super) fn clear_typing_window(map: &mut HashMap<ChatId, Instant>, chat: ChatId) -> bool {
    map.remove(&chat).is_some()
}

/// T-102 pure helper: drop entries whose deadline has passed and return
/// the chats whose windows are still active at `now`. The refresh loop
/// uses the returned list to issue another `sendChatAction` round.
pub(super) fn refresh_typing_windows(
    map: &mut HashMap<ChatId, Instant>,
    now: Instant,
) -> Vec<ChatId> {
    map.retain(|_, deadline| *deadline > now);
    map.keys().copied().collect()
}

/// Parsed reaction payload (T-086-E). The MCP layer writes
/// `{"telegram_msg_id": <i64>, "emoji": "<str>"}`; this turns it back into a
/// typed pair the dispatcher hands to `setMessageReaction`. Returns `None`
/// when either field is missing or the wrong shape — the dispatcher's
/// fallback then logs and skips rather than calling Telegram with bogus
/// args.
pub(super) struct ReactionPayload {
    pub(super) telegram_msg_id: i64,
    pub(super) emoji: String,
}

pub(super) fn parse_reaction_payload(payload: &str) -> Option<ReactionPayload> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let telegram_msg_id = v.get("telegram_msg_id")?.as_i64()?;
    let emoji = v.get("emoji")?.as_str()?.to_string();
    Some(ReactionPayload {
        telegram_msg_id,
        emoji,
    })
}

/// #479: build the `sendRichMessage` request body. Markdown passes straight
/// through; Telegram renders the block tree server-side (verified, spike #478).
pub(super) fn rich_message_body(chat_id: i64, markdown: &str) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "rich_message": { "markdown": markdown },
    })
}

/// #479: send `markdown` to `chat` via Bot API 10.1 `sendRichMessage`, raw HTTP
/// (no teloxide support — see #477). Returns Err on any non-2xx / transport
/// error so the caller can fall back to the HTML sender.
///
/// This path does not thread replies: spike #478 verified only `chat_id` and
/// `rich_message.markdown`, and an unverified reply field could 400 every reply.
/// Callers must not route a reply here — `use_rich_path` sends any row carrying
/// a reply target via the HTML sender instead, so threading is preserved there.
/// Probing a reply field on `sendRichMessage` is a tracked follow-up (#444).
pub(super) async fn forward_rich_row(
    bot: &Bot,
    chat: ChatId,
    markdown: &str,
    row_id: i64,
) -> Result<Option<MessageId>, String> {
    // The token lives in the URL path (Bot API has no header auth). `api_url()`
    // ends in `/` for the default base, so this concat yields the correct
    // `.../bot<token>/sendRichMessage`. (teamctl never sets a custom api_url; a
    // path-bearing custom base would instead need teloxide's join semantics.)
    let url = format!("{}bot{}/sendRichMessage", bot.api_url(), bot.token());
    let body =
        serde_json::to_string(&rich_message_body(chat.0, markdown)).map_err(|e| e.to_string())?;
    let resp = bot
        .client()
        .post(url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        // `without_url()` is mandatory here: the URL carries the bot token, and
        // a reqwest transport error's Display appends "for url (<url>)" — so a
        // bare `e.to_string()` would leak the token into the logs. Stripping the
        // URL keeps the underlying cause (timeout/connect/…) but drops the token.
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let response = serde_json::from_str::<serde_json::Value>(&text).ok();
    let accepted = response
        .as_ref()
        .and_then(|body| body.get("ok").and_then(serde_json::Value::as_bool));
    if status.is_success() && accepted == Some(true) {
        Ok(response
            .and_then(|body| body.get("result")?.get("message_id")?.as_i64())
            .and_then(|id| i32::try_from(id).ok())
            .map(MessageId))
    } else {
        Err(format!(
            "row {row_id}: sendRichMessage HTTP {status}: {text}"
        ))
    }
}

/// Existing HTML text send (render_html + parse_mode=Html + reply threading).
/// Used as the default path and as the #479 rich-send fallback.
pub(super) async fn send_text_html(
    bot: &Bot,
    chat: ChatId,
    body: String,
    reply: Option<ReplyParameters>,
    _row_id: i64,
) -> Result<Option<MessageId>, String> {
    let mut req = bot.send_message(chat, body).parse_mode(ParseMode::Html);
    if let Some(rp) = reply {
        req = req.reply_parameters(rp);
    }
    req.await
        .map(|message| Some(message.id))
        .map_err(|error| error.to_string())
}

/// Decide whether an outbound Text row takes the rich (`sendRichMessage`) path.
/// Rich is used only when (a) rich messages are enabled and (b) the row has no
/// reply target. A reply must stay on the HTML/teloxide sender to preserve
/// Telegram threading, which `sendRichMessage` cannot do yet (spike #478) — so
/// the agent's reply-or-not choice selects the path per message (owner directive
/// tg 2479). The `--no-rich-messages` kill switch (`rich_enabled == false`)
/// forces the HTML sender for every message regardless of reply target.
pub(super) fn use_rich_path(rich_enabled: bool, has_reply_target: bool) -> bool {
    rich_enabled && !has_reply_target
}

const TEXT_MAX_UTF16: usize = 4096;
const CAPTION_MAX_UTF16: usize = 1024;

fn attributed_html(text: &str, sender: &str, max: usize) -> String {
    let rendered = render_html(text);
    let attribution = format!("\n\n— replied by {sender}");
    if text.encode_utf16().count() + attribution.encode_utf16().count() <= max {
        format!("{rendered}\n\n— replied by {}", html_escape_str(sender))
    } else {
        rendered
    }
}

fn attributed_markdown(text: &str, sender: &str) -> String {
    let attribution = format!("\n\n— replied by {}", markdown_escape_str(sender));
    if text.encode_utf16().count() + attribution.encode_utf16().count() <= TEXT_MAX_UTF16 {
        format!("{text}{attribution}")
    } else {
        text.to_string()
    }
}

/// Split raw text before rendering so HTML tags and Unicode scalars are
/// never cut in half. Oversized CLI messages retain their complete body.
fn text_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units + ch.len_utf16() > TEXT_MAX_UTF16 {
            chunks.push(&text[start..index]);
            start = index;
            units = 0;
        }
        units += ch.len_utf16();
    }
    chunks.push(&text[start..]);
    chunks
}

async fn send_text_row(
    bot: &Bot,
    chat: ChatId,
    text: &str,
    sender: &str,
    reply: Option<ReplyParameters>,
    row_id: i64,
    rich: bool,
) -> Result<Option<MessageId>, String> {
    let mut sent = None;
    for chunk in text_chunks(text) {
        let html_body = attributed_html(chunk, sender, TEXT_MAX_UTF16);
        if use_rich_path(rich, reply.is_some()) {
            let markdown = attributed_markdown(chunk, sender);
            sent = match forward_rich_row(bot, chat, &markdown, row_id).await {
                Ok(id) => id,
                Err(error) => {
                    tracing::warn!(%error, "rich send failed; falling back to HTML");
                    send_text_html(bot, chat, html_body, reply.clone(), row_id).await?
                }
            };
        } else {
            sent = send_text_html(bot, chat, html_body, reply.clone(), row_id).await?;
        }
    }
    Ok(sent)
}

pub(super) async fn forward_row(
    bot: &Bot,
    chat: ChatId,
    row: &MailboxRow,
    rich: bool,
) -> Result<Option<MessageId>, String> {
    let kind = classify_kind(row.kind.as_deref());
    let attribution = format!("\n\n— replied by {}", html_escape_str(&row.sender));
    let reply = reply_parameters_for(row.telegram_msg_id);
    match kind {
        DispatchKind::Text => {
            return send_text_row(bot, chat, &row.text, &row.sender, reply, row.id, rich).await;
        }
        DispatchKind::Image | DispatchKind::File => {
            let Some(payload) = row.payload.as_deref().and_then(parse_payload) else {
                return send_text_html(
                    bot,
                    chat,
                    format!(
                        "{} (media payload unparseable){attribution}",
                        render_html(&row.text)
                    ),
                    reply,
                    row.id,
                )
                .await;
            };
            let Some(input) = input_file_from(&payload) else {
                return send_text_html(
                    bot,
                    chat,
                    format!(
                        "{} (unsupported media source <code>{}</code>){attribution}",
                        render_html(&row.text),
                        html_escape_str(&payload.source)
                    ),
                    reply,
                    row.id,
                )
                .await;
            };
            let caption = payload
                .caption
                .as_deref()
                .map(|caption| attributed_html(caption, &row.sender, CAPTION_MAX_UTF16))
                .unwrap_or_else(|| {
                    attributed_html("", &row.sender, CAPTION_MAX_UTF16)
                        .trim_start()
                        .to_string()
                });
            let result = if kind == DispatchKind::Image {
                let mut request = bot
                    .send_photo(chat, input)
                    .caption(caption)
                    .parse_mode(ParseMode::Html);
                if let Some(reply) = reply {
                    request = request.reply_parameters(reply);
                }
                request.await
            } else {
                let mut request = bot
                    .send_document(chat, input)
                    .caption(caption)
                    .parse_mode(ParseMode::Html);
                if let Some(reply) = reply {
                    request = request.reply_parameters(reply);
                }
                request.await
            };
            return result
                .map(|message| Some(message.id))
                .map_err(|error| error.to_string());
        }
        DispatchKind::Reaction => {
            let reaction = row
                .payload
                .as_deref()
                .and_then(parse_reaction_payload)
                .ok_or_else(|| format!("row {}: reaction payload unparseable", row.id))?;
            bot.set_message_reaction(chat, MessageId(reaction.telegram_msg_id as i32))
                .reaction(vec![ReactionType::Emoji {
                    emoji: reaction.emoji,
                }])
                .await
                .map_err(|error| error.to_string())?;
        }
        DispatchKind::Typing => {}
        DispatchKind::UnknownFallback => {
            return send_text_row(bot, chat, &row.text, &row.sender, reply, row.id, false).await;
        }
    }
    Ok(None)
}
