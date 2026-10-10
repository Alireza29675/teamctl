use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use rusqlite::params;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, ParseMode};

use crate::{
    approval::{approval_keyboard, decode_options},
    render::{html_escape_str, render_html},
    repository::{mark_approval_delivered, outgoing_messages, pending_approvals, should_route},
    transport::{
        classify_kind, clear_typing_window, extend_typing_window, forward_row,
        refresh_typing_windows, DispatchKind,
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
    let mut delivery_failed = false;
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
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            forward_row(bot, chat, &row, state.rich).await
        };
        match result {
            Ok(()) => {
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
