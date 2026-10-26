use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use rusqlite::params;
use team_core::chat::{Delivery, Repository};
use teloxide::prelude::*;
use teloxide::types::{ChatAction, ParseMode};

use crate::{
    approval::{approval_keyboard, decode_options},
    render::{html_escape_str, render_html},
    repository::{mark_approval_delivered, outgoing_messages, pending_approvals, should_route},
    transport::{
        classify_kind, clear_typing_window, extend_typing_window, forward_row,
        refresh_typing_windows, DispatchKind, MailboxRow,
    },
    State, TYPING_REFRESH_INTERVAL, TYPING_WINDOW_CEILING,
};

/// Retry persisted work after a restart or transport failure. A successful
/// send is the only event that consumes an outbound mailbox row or card.
pub(super) async fn outbound_loop(bot: Bot, state: Arc<State>) {
    let Some(&primary) = state.allow.first() else {
        tracing::warn!("no authorized_chat_ids — outbound disabled");
        return;
    };
    let poll_interval = Duration::from_millis(500);
    let mut retry_delay = poll_interval;
    loop {
        tokio::time::sleep(retry_delay).await;
        if let Err(error) = outbound_once(&bot, &state, ChatId(primary)).await {
            tracing::warn!(%error, "outbound poll failed; pending work will be retried");
            retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
        } else {
            retry_delay = poll_interval;
        }
    }
}

pub(super) async fn outbound_once(bot: &Bot, state: &State, chat: ChatId) -> Result<()> {
    let mut delivery_failed = match chat_outbound_once(bot, state).await {
        Ok(()) => false,
        Err(error) => {
            tracing::warn!(%error, "conversation delivery failed");
            true
        }
    };
    let approvals = {
        let conn = state.conn.lock().await;
        pending_approvals(&conn, state.manager_project())?
            .into_iter()
            .filter(|(_, agent, _, _, _)| should_route(state.manager.as_deref(), agent, &conn))
            .collect::<Vec<_>>()
    };
    for (id, agent, action, summary, options_json) in approvals {
        let keyboard = approval_keyboard(id, &decode_options(options_json.as_deref()));
        let text = format!(
            "🔐 #{id}  {}\naction: {}\n{}",
            html_escape_str(&agent),
            html_escape_str(&action),
            render_html(&summary),
        );
        match bot
            .send_message(chat, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await
        {
            Ok(message) => {
                let delivered = {
                    let conn = state.conn.lock().await;
                    mark_approval_delivered(&conn, id)
                };
                if !matches!(delivered, Ok(true)) {
                    // The request may have expired or been resolved while the
                    // transport was in flight. Remove this late card rather
                    // than exposing buttons for a decision we cannot accept.
                    if let Err(error) = bot.delete_message(chat, message.id).await {
                        tracing::warn!(%error, id, "could not delete late approval card");
                        if let Err(error) = bot
                            .edit_message_reply_markup(chat, message.id)
                            .reply_markup(teloxide::types::InlineKeyboardMarkup::new(
                                Vec::<Vec<_>>::new(),
                            ))
                            .await
                        {
                            tracing::warn!(%error, id, "could not disable late approval card");
                        }
                    }
                }
                delivered?;
            }
            Err(error) => {
                delivery_failed = true;
                tracing::warn!(%error, id, "approval delivery failed; retaining card");
            }
        }
    }
    let messages = {
        let conn = state.conn.lock().await;
        outgoing_messages(&conn, state.manager_project())?
            .into_iter()
            .filter(|row| should_route(state.manager.as_deref(), &row.sender, &conn))
            .collect::<Vec<_>>()
    };
    for row in messages {
        let kind = classify_kind(row.kind.as_deref());
        match kind {
            DispatchKind::Text | DispatchKind::Image | DispatchKind::File => {
                clear_typing_window(&mut *state.typing.lock().await, chat);
            }
            DispatchKind::Typing => {
                extend_typing_window(
                    &mut *state.typing.lock().await,
                    chat,
                    Instant::now(),
                    TYPING_WINDOW_CEILING,
                );
            }
            _ => {}
        }
        let result = if matches!(kind, DispatchKind::Typing) {
            bot.send_chat_action(chat, ChatAction::Typing)
                .await
                .map(|_| None)
                .map_err(|error| error.to_string())
        } else {
            forward_row(bot, chat, &row, state.rich).await
        };
        match result {
            Ok(_) => {
                let conn = state.conn.lock().await;
                conn.execute(
                    "UPDATE messages SET acked_at=strftime('%s','now') WHERE id=?1",
                    params![row.id],
                )?;
            }
            Err(error) => {
                delivery_failed = true;
                tracing::warn!(%error, id=row.id, "message delivery failed; retaining row")
            }
        }
    }
    if delivery_failed {
        bail!("one or more outbound deliveries failed");
    }
    Ok(())
}

/// Claim one delivery at a time so a slow provider cannot expire a batch's
/// later leases before they are sent. Legacy rows and approvals keep their
/// separate fallback path above because their peer was never persisted.
pub(super) async fn chat_outbound_once(bot: &Bot, state: &State) -> Result<()> {
    let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let owner = format!("telegram:{}:{}", std::process::id(), clock.as_nanos());
    let deliveries = {
        let conn = state.conn.lock().await;
        crate::chat::register_capabilities(&conn, &state.adapter_id())?;
        Repository::new(&conn).claim_deliveries(
            &state.adapter_id(),
            &owner,
            clock.as_secs_f64(),
            120.0,
            1,
        )?
    };
    for delivery in deliveries {
        let agent = {
            let conn = state.conn.lock().await;
            Repository::new(&conn)
                .get_conversation(&delivery.conversation_id)?
                .agent_id
        };
        let routed = delivery_row(state, &delivery, &agent).and_then(|(chat, row)| {
            let window = if classify_kind(row.kind.as_deref()) == DispatchKind::Typing {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs_f64();
                Some(typing_window(&delivery, now)?)
            } else {
                None
            };
            Ok((chat, row, window))
        });
        let (chat, row, window) = match routed {
            Ok(routed) => routed,
            Err(error) => {
                let conn = state.conn.lock().await;
                Repository::new(&conn).mark_failed(
                    delivery.id,
                    &owner,
                    &error.to_string(),
                    false,
                )?;
                continue;
            }
        };
        let kind = classify_kind(row.kind.as_deref());
        if matches!(
            kind,
            DispatchKind::Text | DispatchKind::Image | DispatchKind::File
        ) {
            clear_typing_window(&mut *state.typing.lock().await, chat);
        }
        let timeout = window.unwrap_or(Duration::from_secs(60));
        let typing_deadline = window.map(|window| Instant::now() + window);
        let result = tokio::time::timeout(timeout, async {
            if window.is_some() {
                bot.send_chat_action(chat, ChatAction::Typing)
                    .await
                    .map(|_| None)
                    .map_err(|error| error.to_string())
            } else {
                forward_row(bot, chat, &row, state.rich).await
            }
        })
        .await;
        if let Some(deadline) = typing_deadline.filter(|_| matches!(result, Ok(Ok(_)))) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs_f64();
            if let Ok(remaining) = typing_window(&delivery, now) {
                let remaining = remaining.min(deadline.saturating_duration_since(Instant::now()));
                if !remaining.is_zero() {
                    extend_typing_window(
                        &mut *state.typing.lock().await,
                        chat,
                        Instant::now(),
                        remaining,
                    );
                }
            }
        }
        let conn = state.conn.lock().await;
        let repository = Repository::new(&conn);
        match result {
            Ok(Ok(message_id)) => {
                let external_id = message_id.map(|id| id.0.to_string());
                repository.mark_sent(delivery.id, &owner, external_id.as_deref())?;
            }
            result => {
                let error = match result {
                    Ok(Err(error)) => error
                        .chars()
                        .filter(|ch| *ch != '\0')
                        .take(1024)
                        .collect::<String>(),
                    Err(_) => "Telegram delivery timed out".into(),
                    Ok(Ok(_)) => unreachable!(),
                };
                repository.mark_failed(delivery.id, &owner, &error, true)?;
                bail!("Telegram delivery {} failed: {error}", delivery.id);
            }
        }
    }
    Ok(())
}

pub(super) fn delivery_row(
    state: &State,
    delivery: &Delivery,
    agent: &str,
) -> Result<(ChatId, MailboxRow)> {
    if delivery.adapter_id != state.adapter_id() {
        bail!("delivery belongs to another Telegram adapter");
    }
    let peer: i64 = delivery
        .peer_id
        .parse()
        .context("invalid Telegram peer ID")?;
    if delivery.peer_id != peer.to_string() || !state.is_authorized(peer) {
        bail!("Telegram peer is not authorized");
    }
    if !matches!(
        delivery.message.kind.as_str(),
        "text" | "image" | "file" | "typing" | "reaction"
    ) {
        bail!(
            "Telegram adapter does not support outbound kind {}",
            delivery.message.kind
        );
    }
    let reply_to = delivery
        .reply_to_external_id
        .as_deref()
        .map(str::parse::<i32>)
        .transpose()
        .context("invalid Telegram reply message ID")?;
    Ok((
        ChatId(peer),
        MailboxRow {
            id: delivery.message.id,
            sender: agent.to_string(),
            text: delivery.message.text.clone(),
            kind: Some(delivery.message.kind.clone()),
            payload: delivery
                .message
                .payload
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            telegram_msg_id: reply_to.map(i64::from),
        },
    ))
}

fn typing_window(delivery: &Delivery, now: f64) -> Result<Duration> {
    let expires_at = delivery
        .message
        .payload
        .as_ref()
        .and_then(|payload| payload.get("expires_at"))
        .and_then(serde_json::Value::as_f64)
        .filter(|expires_at| expires_at.is_finite())
        .context("typing delivery requires a finite expires_at timestamp")?;
    if expires_at <= now {
        bail!("typing indicator expired before delivery");
    }
    Ok(Duration::from_secs_f64(
        (expires_at - now).min(TYPING_WINDOW_CEILING.as_secs_f64()),
    ))
}

/// T-102 background task: every `TYPING_REFRESH_INTERVAL`, drop expired
/// per-chat windows and re-fire `sendChatAction("typing")` on whatever
/// remains. Telegram's typing indicator persists ~5s natively, so a 4s
/// refresh keeps the bubble visible without gaps. The ceiling
/// (`TYPING_WINDOW_CEILING`) bounds the refresh — once `now` passes the
/// stored deadline the entry is dropped, the indicator naturally
/// expires on the Telegram side, and the chat goes quiet.
pub(super) async fn typing_refresh_loop(bot: Bot, state: Arc<State>) {
    loop {
        tokio::time::sleep(TYPING_REFRESH_INTERVAL).await;
        let active: Vec<ChatId> = {
            let mut map = state.typing.lock().await;
            refresh_typing_windows(&mut map, Instant::now())
        };
        for chat in active {
            if let Err(e) = bot.send_chat_action(chat, ChatAction::Typing).await {
                tracing::warn!("typing refresh send_chat_action failed for {chat}: {e}");
            }
        }
    }
}
