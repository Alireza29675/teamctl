//! Bridge Telegram's native mailbox flow to durable, peer-scoped conversations.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use team_core::chat::{ChatMessage, InboundMessage, Repository};
use teloxide::types::Message;

use crate::State;

impl State {
    pub(super) fn adapter_id(&self) -> String {
        match self.manager.as_deref() {
            Some(manager) => format!("telegram:{manager}"),
            None => "telegram:legacy".into(),
        }
    }
}

pub(super) fn register_capabilities(conn: &Connection, adapter: &str) -> Result<()> {
    let capabilities = ["text", "image", "file", "typing", "reaction"].map(str::to_string);
    Repository::new(conn).register_capabilities(adapter, &capabilities)?;
    Ok(())
}

pub(super) fn inbound_message(
    state: &State,
    msg: &Message,
    target: &str,
    text: &str,
    kind: &str,
    payload: Option<Value>,
) -> Result<InboundMessage> {
    if !state.is_authorized(msg.chat.id.0) {
        bail!("Telegram chat is not authorized");
    }
    // Anonymous group administrators use sender_chat as their real identity;
    // Telegram may also provide a shared GroupAnonymousBot proxy in `from`.
    // Keep that chat identity distinct from ordinary numeric human IDs.
    let sender_id = match msg.sender_chat.as_ref() {
        Some(chat) => format!("chat:{}", chat.id.0),
        None => msg
            .from
            .as_ref()
            .filter(|sender| !sender.is_bot)
            .context("Telegram message has no human or sender-chat identity")?
            .id
            .0
            .to_string(),
    };
    Ok(InboundMessage {
        adapter_id: state.adapter_id(),
        peer_id: msg.chat.id.0.to_string(),
        sender_id,
        agent_id: target.to_string(),
        external_id: msg.id.0.to_string(),
        text: text.to_string(),
        kind: kind.to_string(),
        payload,
        title: Some(msg.chat.id.0.to_string()),
        reply_to_external_id: msg.reply_to_message().map(|reply| reply.id.0.to_string()),
    })
}

/// Preserve Telegram's legacy sender/threading fields and publish the chat
/// route in the same commit. A replay deletes its untouched duplicate row.
pub(super) fn record_text(
    conn: &Connection,
    inbound: InboundMessage,
    telegram_id: i64,
    delivery_mode: Option<&str>,
) -> Result<ChatMessage> {
    let project = inbound
        .agent_id
        .split_once(':')
        .map(|(project, _)| project)
        .context("Telegram target must be a canonical agent ID")?;
    let tx = conn.unchecked_transaction()?;
    register_capabilities(&tx, &inbound.adapter_id)?;
    tx.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, kind, structured_payload,
             sent_at, telegram_msg_id, delivery_mode)
         VALUES (?1, 'user:telegram', ?2, ?3, ?4, ?5, strftime('%s','now'), ?6, ?7)",
        params![
            project,
            inbound.agent_id,
            inbound.text,
            inbound.kind,
            inbound
                .payload
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            telegram_id,
            delivery_mode
        ],
    )?;
    let message = Repository::new(&tx).record_existing_inbound(tx.last_insert_rowid(), inbound)?;
    tx.commit()?;
    Ok(message)
}

/// Media paths depend on the staging ID. Detect a completed source before
/// allocating another path or repeating a download/transcription on replay.
pub(super) fn source_recorded(conn: &Connection, inbound: &InboundMessage) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM chat_sources WHERE adapter_id=?1 AND peer_id=?2 AND external_id=?3",
            params![inbound.adapter_id, inbound.peer_id, inbound.external_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Provider message IDs are only unique within a chat. Unknown legacy peers
/// stay unresolved rather than attributing another chat's mailbox row.
pub(super) fn lookup_replied_row(
    conn: &Connection,
    adapter: &str,
    peer: &str,
    telegram_id: i64,
) -> Result<Option<(Option<i64>, String)>> {
    Ok(conn
        .query_row(
            "SELECT cm.mailbox_id,
                    CASE WHEN cm.direction='human' THEN 'user:telegram' ELSE cc.agent_id END
             FROM chat_messages cm JOIN chat_conversations cc ON cc.id=cm.conversation_id
             WHERE cc.adapter_id=?1 AND cc.peer_id=?2 AND cm.external_id=?3
             ORDER BY cm.id DESC LIMIT 1",
            params![adapter, peer, telegram_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}
