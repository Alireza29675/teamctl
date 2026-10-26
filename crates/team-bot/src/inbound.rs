use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ParseMode;

use crate::{
    chat::{inbound_message, lookup_replied_row, record_text},
    media::handle_inbound_media,
    render::{html_escape_str, render_html},
    repository::{active_approvals, agent_runtime, should_route},
    tmux::{slash_outcome, tmux_send_keys, SlashOutcome},
    voice::{handle_voice, handle_voice_stt_missing},
    State,
};

/// T-104: detect a leading `/readnow ` prefix on a Telegram body and split
/// it from the routable text. Returns `(text, delivery_mode)` where
/// `delivery_mode` is `Some("immediate")` iff the prefix matched (case-
/// sensitive, single-space separator) and `None` otherwise. Empty body
/// after the prefix still returns `Some("immediate")` so the caller can
/// decide how to handle the empty-payload case.
pub(super) fn peel_readnow(body: &str) -> (&str, Option<&'static str>) {
    if let Some(rest) = body.strip_prefix("/readnow ") {
        (rest, Some("immediate"))
    } else {
        (body, None)
    }
}

/// Per-message budget on quoted text in an inbound reply prefix (#334). The
/// agent gets the gist; the mailbox id in the header is the recovery path for
/// the full original. Counted in chars (not bytes) so a multibyte tail is
/// never split mid-codepoint.
pub(super) const INBOUND_QUOTE_MAX_CHARS: usize = 500;

/// The replied-to / quoted context for an inbound Telegram message (#334),
/// resolved against the mailbox so the agent sees what the user is replying
/// to. Built by [`resolve_reply_context`]; rendered by [`build_inbound_body`].
pub(super) struct ReplyContext {
    /// Who sent the replied-to message: the mailbox `sender` of the referenced
    /// row when found (`user:telegram` or a `<project>:<agent>` id), else a
    /// Telegram-derived fallback (`user:telegram` / `agent` / `unknown`).
    pub(super) sender: String,
    /// Mailbox row id of the replied-to message, when it resolves. Omitted from
    /// the header otherwise — the agent loses the cross-reference handle but
    /// still sees the quoted text.
    pub(super) mailbox_id: Option<i64>,
    /// The quoted text, already truncated: the user's partial selection
    /// (Telegram `quote`) when present, else the replied-to message's text or
    /// caption. `None` when there is nothing quotable.
    pub(super) quoted: Option<String>,
}

/// Truncate quoted text to [`INBOUND_QUOTE_MAX_CHARS`] chars, appending an
/// ellipsis when it was cut. Char-based so a multibyte boundary is never split.
pub(super) fn truncate_quote(text: &str) -> String {
    let mut out: String = text.chars().take(INBOUND_QUOTE_MAX_CHARS).collect();
    if text.chars().count() > INBOUND_QUOTE_MAX_CHARS {
        out.push('…');
    }
    out
}

/// Classify a replied-to message's sender from the Telegram `from.is_bot` flag:
/// a bot account is an agent's outbound message, a human is the operator, an
/// absent sender (channel / anonymous admin) is unknown. Pure so the three
/// branches are testable without a teloxide `Message` fixture.
pub(super) fn sender_label_for(is_bot: Option<bool>) -> &'static str {
    match is_bot {
        Some(true) => "agent",
        Some(false) => "user:telegram",
        None => "unknown",
    }
}

/// Fallback sender label for a replied-to message not found in the mailbox.
pub(super) fn telegram_sender_label(replied: &Message) -> String {
    sender_label_for(replied.from.as_ref().map(|u| u.is_bot)).to_string()
}

/// Build the [`ReplyContext`] for an inbound message when the user replied to
/// or quoted an earlier message (#334). Returns `None` for an ordinary message
/// (no reply, no quote) so the body forwards unchanged with zero DB work. Best
/// effort: an unresolved row degrades to a Telegram-derived sender and no
/// mailbox id, never an error.
pub(super) async fn resolve_reply_context(msg: &Message, state: &State) -> Option<ReplyContext> {
    let replied = msg.reply_to_message();
    let quote = msg.quote();
    if replied.is_none() && quote.is_none() {
        return None;
    }

    // Sender + mailbox id come from the replied-to message; the mailbox row,
    // when found, is authoritative for the sender label.
    let (sender, mailbox_id) = match replied {
        Some(r) => {
            let row = {
                let c = state.conn.lock().await;
                lookup_replied_row(
                    &c,
                    &state.adapter_id(),
                    &msg.chat.id.0.to_string(),
                    i64::from(r.id.0),
                )
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "inbound reply lookup failed");
                    None
                })
            };
            match row {
                Some((id, sender)) => (sender, id),
                None => (telegram_sender_label(r), None),
            }
        }
        None => ("unknown".to_string(), None),
    };

    // Quoted text: prefer the user's explicit selection (`quote`), else the
    // replied-to message's own text or caption. Empty is treated as nothing
    // quotable (`None`) so the field's invariant holds and the header renders
    // alone rather than with a stray empty blockquote line.
    let quoted = quote
        .map(|q| q.text.as_str())
        .or_else(|| replied.and_then(|r| r.text().or_else(|| r.caption())))
        .filter(|s| !s.is_empty())
        .map(truncate_quote);

    Some(ReplyContext {
        sender,
        mailbox_id,
        quoted,
    })
}

/// Prefix an inbound mailbox body with a Markdown blockquote carrying the
/// replied-to / quoted context (#334), so the agent reads what the user is
/// referring back to as part of the body. Returns `new_text` unchanged for an
/// ordinary message (`ctx` is `None`). Plain text, not HTML: the mailbox body
/// is read raw by the agent, and `>` is the Markdown blockquote the model
/// already understands.
pub(super) fn build_inbound_body(new_text: &str, ctx: Option<&ReplyContext>) -> String {
    let Some(ctx) = ctx else {
        return new_text.to_string();
    };
    let mut out = String::new();
    match ctx.mailbox_id {
        Some(id) => out.push_str(&format!("> [reply to {} msg {}]\n", ctx.sender, id)),
        None => out.push_str(&format!("> [reply to {}]\n", ctx.sender)),
    }
    if let Some(quoted) = &ctx.quoted {
        for line in quoted.lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(new_text);
    out
}

pub(super) async fn handle_message(
    bot: Bot,
    msg: Message,
    state: Arc<State>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id.0;
    let trimmed = msg.text().map(str::trim).unwrap_or("");

    // Bootstrap: a chat that isn't on the allow list gets a one-shot reply
    // to `/start` exposing its own chat id, so the operator can paste it
    // into `.env` without hunting for @userinfobot.
    if !state.allow.contains(&chat_id) && trimmed == "/start" {
        bot.send_message(
            msg.chat.id,
            format!(
                "This chat isn't authorized yet.\n\n\
                 Your chat id: {chat_id}\n\n\
                 Add it to .env next to your team-compose.yaml:\n\
                 TEAMCTL_TELEGRAM_CHATS={chat_id}\n\n\
                 Then restart team-bot."
            ),
        )
        .await?;
        return Ok(());
    }

    if !state.is_authorized(chat_id) {
        return Ok(());
    }
    // T-086-C inbound media: photos and documents arrive with `msg.text()`
    // empty (the caption sits on `msg.caption()` instead). Detect before the
    // text-routing chain — without this, media messages would silently fall
    // through every arm and the operator would see no acknowledgement.
    if msg.photo().is_some() || msg.document().is_some() {
        return handle_inbound_media(&bot, &msg, &state).await;
    }
    // T-101: inbound voice note. Detect before the text-routing chain since
    // `msg.text()` is empty on a voice message — without this, voice would
    // silently fall through every arm. Voice handling requires both an STT
    // runtime and a manager-scoped bot (we need someone to route the
    // transcript to). Either missing → fall through (preserves prior
    // behavior: voice messages stay unhandled).
    if msg.voice().is_some() && state.stt.is_some() && state.manager.is_some() {
        return handle_voice(&bot, &msg, &state).await;
    }
    // T-236: voice arrived but STT isn't configured on this manager-scoped
    // bot. Reply with a config hint rather than silently dropping —
    // operator otherwise can't tell whether the voice was received,
    // intentionally ignored, or their config is broken. Unscoped bots
    // (`state.manager.is_none()`) keep today's silent fall-through; their
    // inbound handling is intentionally minimal and out of scope here.
    if msg.voice().is_some() && state.stt.is_none() && state.manager.is_some() {
        return handle_voice_stt_missing(&bot, &msg).await;
    }
    // Capture the inbound Telegram message id on every mailbox row we
    // write. T-086-B feeds it to `react_to_user.telegram_msg_id` for
    // emoji reactions. T-168 also has the store look it up server-side
    // when an agent's `reply_to_user.reply_to_message_id` (= a mailbox
    // id) references this row, resolving to the value persisted here.
    // #334: when the user tap-replied to (or quoted) an earlier message,
    // resolve that context once so every mailbox-writing arm below can prefix
    // the body with it. `None` for an ordinary message (no extra DB work).
    let reply_ctx = resolve_reply_context(&msg, &state).await;
    if let Some(rest) = trimmed.strip_prefix("/dm ") {
        if let Some((target, body)) = rest.split_once(' ') {
            if target.split_once(':').is_some() {
                // T-104: `/readnow ` on the body bypasses lazy delivery so
                // the message lands inline in the agent's input stream
                // instead of as a stub. Single-space-separated, case-
                // sensitive prefix; stripped before insert.
                let (body, delivery_mode) = peel_readnow(body);
                let body = build_inbound_body(body, reply_ctx.as_ref());
                forward_text_to(
                    &bot,
                    &msg,
                    &state,
                    target,
                    &body,
                    delivery_mode,
                    format!("→ {target}"),
                )
                .await?;
            }
        }
    } else if !trimmed.is_empty() && !trimmed.starts_with('/') && state.manager.is_some() {
        // Plain text on a manager-scoped bot: route the message to the
        // bot's manager. The whole point of `teamctl bot setup`'s 1:1
        // mapping is that DMing the bot reaches the matching manager
        // without `/dm role text` ceremony.
        let target = state.manager.as_deref().unwrap();
        if target.split_once(':').is_some() {
            let body = build_inbound_body(trimmed, reply_ctx.as_ref());
            forward_text_to(
                &bot,
                &msg,
                &state,
                target,
                &body,
                None,
                format!("→ {target}"),
            )
            .await?;
        }
    } else if trimmed.starts_with("/readnow ") && state.manager.is_some() {
        // T-104: plain-text `/readnow ` on a manager-scoped bot. Strip the
        // prefix and route to the manager with `delivery_mode='immediate'`
        // so the body lands inline in the agent's context rather than as a
        // stub. Sits as its own arm (not folded into the plain-text arm
        // above) because Telegram's command parser treats anything starting
        // with `/` as a slash command.
        let target = state.manager.as_deref().unwrap();
        let (body, delivery_mode) = peel_readnow(trimmed);
        if !body.is_empty() && target.split_once(':').is_some() {
            let body = build_inbound_body(body, reply_ctx.as_ref());
            forward_text_to(
                &bot,
                &msg,
                &state,
                target,
                &body,
                delivery_mode,
                format!("→ {target} (now)"),
            )
            .await?;
        }
    } else if trimmed == "/pending" {
        let rows = {
            let conn = state.conn.lock().await;
            active_approvals(&conn, state.manager_project(), false).map(|rows| {
                rows.into_iter()
                    .filter(|(_, agent, _, _, _)| {
                        should_route(state.manager.as_deref(), agent, &conn)
                    })
                    .collect::<Vec<_>>()
            })
        };
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "pending approvals could not be read");
                bot.send_message(
                    msg.chat.id,
                    "internal error: could not read pending approvals; please retry.",
                )
                .await?;
                return Ok(());
            }
        };
        if rows.is_empty() {
            bot.send_message(msg.chat.id, "No pending approvals.")
                .await?;
        } else {
            let mut out = String::from("Pending approvals:\n");
            for (id, agent, action, summary, _) in rows {
                out.push_str(&format!(
                    "#{id} {} · {}: {}\n",
                    html_escape_str(&agent),
                    html_escape_str(&action),
                    render_html(&summary),
                ));
            }
            bot.send_message(msg.chat.id, out)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    } else if trimmed == "/start" || trimmed == "/help" {
        // T-367: keep the first-contact greeting a short one-liner — just
        // confirm who you're talking to. The power-user commands (/pending,
        // /dm, slash-passthrough) still work; they live on `/help` now
        // instead of crowding the greeting. Both commands are matched here,
        // ahead of the slash-passthrough arm below, so `/help` never gets
        // typed into the manager's tmux session.
        let name = state
            .manager
            .as_deref()
            .map(|mgr| state.manager_display_name.as_deref().unwrap_or(mgr));
        let body = start_help_body(trimmed == "/help", name);
        bot.send_message(msg.chat.id, body).await?;
    } else if trimmed.starts_with('/') && state.manager.is_some() {
        // T-086-G slash-passthrough: any unrecognised slash command on a
        // manager-scoped bot gets typed straight into the manager's tmux
        // session via `tmux send-keys`. Feature-gated on slash-REPL runtimes
        // (claude-code, codex) per Decision 6 (manager-only routing). Trust
        // posture is "operator
        // owns the bot" per Decision 7 — no allowlist on slash content; the
        // bot is per-operator and chat-id-gated, the trust boundary is the
        // same as the operator's existing `tmux attach` access.
        let manager = state.manager.as_deref().unwrap();
        let runtime_opt = {
            let c = state.conn.lock().await;
            agent_runtime(&c, manager)
        };
        let Some(runtime) = runtime_opt else {
            bot.send_message(
                msg.chat.id,
                format!("unknown manager `{manager}` — slash-passthrough aborted"),
            )
            .await?;
            return Ok(());
        };
        match slash_outcome(manager, &runtime, &state.tmux_prefix) {
            SlashOutcome::Passthrough { session } => match tmux_send_keys(&session, trimmed) {
                Ok(()) => {
                    bot.send_message(msg.chat.id, format!("→ {manager}"))
                        .await?;
                }
                Err(err) => {
                    bot.send_message(msg.chat.id, format!("tmux error: {err}"))
                        .await?;
                }
            },
            SlashOutcome::Reject { reason } => {
                bot.send_message(msg.chat.id, reason).await?;
            }
        }
    }
    Ok(())
}

async fn forward_text_to(
    bot: &Bot,
    msg: &Message,
    state: &State,
    target: &str,
    body: &str,
    delivery_mode: Option<&str>,
    confirmation: String,
) -> ResponseResult<()> {
    let inserted = match inbound_message(state, msg, target, body, "text", None) {
        Ok(inbound) => {
            let conn = state.conn.lock().await;
            record_text(&conn, inbound, i64::from(msg.id.0), delivery_mode).map(|_| 1)
        }
        Err(error) => Err(error),
    };
    confirm_inbound(bot, msg.chat.id, inserted, confirmation).await
}

pub(super) async fn confirm_inbound<E: std::fmt::Display>(
    bot: &Bot,
    chat: ChatId,
    inserted: Result<usize, E>,
    confirmation: String,
) -> ResponseResult<()> {
    let body = match inserted {
        Ok(1) => confirmation,
        Ok(_) => "internal error: could not record the message; please retry.".to_string(),
        Err(error) => {
            tracing::error!(%error, "inbound message was not recorded");
            "internal error: could not record the message; please retry.".to_string()
        }
    };
    bot.send_message(chat, body).await?;
    Ok(())
}

/// T-367: build the body for `/start` (greeting) or `/help` (command list).
///
/// `is_help` selects the command list; otherwise a short one-line greeting.
/// `name` is the label for a manager-scoped bot — the manager's
/// `display_name` (T-160) when set, else the `<project>:<manager>` id — and
/// `None` for an unscoped bot, which has no single manager to name. Pulled
/// out as a free function so the copy is unit-testable without a teloxide
/// `Bot` or an async runtime.
pub(super) fn start_help_body(is_help: bool, name: Option<&str>) -> String {
    match (is_help, name) {
        (true, Some(name)) => format!(
            "teamctl commands:\n\
             /pending — show pending approvals\n\
             /dm <project>:<agent> <text> — send to a different agent (rare)\n\
             /<cmd> — slash-passthrough to {name}'s tmux session (Claude Code and Codex)\n\
             \n\
             Just type a message to chat with {name}."
        ),
        (true, None) => "teamctl commands:\n\
             /dm <project>:<agent> <message> — send a DM\n\
             /pending — show pending approvals"
            .into(),
        (false, Some(name)) => format!("Connected to {name} via teamctl. Just type to chat."),
        (false, None) => "Connected via teamctl. Send /help for commands.".into(),
    }
}
