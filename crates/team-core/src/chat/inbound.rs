use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use super::repository::{content, fingerprint, identity, now};
use super::{ChatMessage, DeliveryStatus, Direction, Error, InboundMessage, Repository, Result};

impl Repository<'_> {
    pub fn ingest(&self, inbound: InboundMessage) -> Result<ChatMessage> {
        validate(&inbound)?;
        self.atomic(|| {
            if let Some(message) = self.source(&inbound)? { return Ok(message); }
            let project = self.require_agent(&inbound.agent_id)?;
            // Hash provider-controlled IDs so they cannot imitate lifecycle
            // sources or inject delimiters into legacy sender conventions.
            let sender = format!("user:chat:{}", fingerprint(&serde_json::json!([inbound.adapter_id, inbound.peer_id, inbound.sender_id])));
            self.conn.execute(
                "INSERT INTO messages(project_id, sender, recipient, text, kind, structured_payload, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![project, sender, inbound.agent_id, inbound.text, inbound.kind, inbound.payload.as_ref().map(serde_json::to_string).transpose()?, now()],
            )?;
            self.record_new_inbound(self.conn.last_insert_rowid(), &inbound)
        })
    }

    /// Attach a native transport's already-created visible inbox row. A
    /// savepoint is nested inside the caller's transaction, so a fresh media
    /// ID and its chat route are committed or rolled back together.
    pub fn record_existing_inbound(
        &self,
        mailbox_id: i64,
        inbound: InboundMessage,
    ) -> Result<ChatMessage> {
        validate(&inbound)?;
        self.atomic(|| {
            let row = self.existing_inbound(mailbox_id)?;
            row.validate(&inbound)?;
            if let Some(message) = self.source(&inbound)? {
                if message.mailbox_id == Some(mailbox_id) {
                    return Ok(message);
                }
                // Telegram inserts before publication. Only an untouched,
                // matching replay may be discarded; never delete a consumed
                // or independently linked inbox row during deduplication.
                if row.chat_id.is_some() || row.conversation_id.is_some() || row.consumed {
                    return Err(Error::IdempotencyConflict);
                }
                self.conn
                    .execute("DELETE FROM messages WHERE id = ?1", [mailbox_id])?;
                return Ok(message);
            }
            if row.chat_id.is_some() || row.conversation_id.is_some() || row.consumed {
                return Err(Error::IdempotencyConflict);
            }
            self.require_agent(&inbound.agent_id)?;
            self.record_new_inbound(mailbox_id, &inbound)
        })
    }

    fn record_new_inbound(&self, mailbox_id: i64, inbound: &InboundMessage) -> Result<ChatMessage> {
        let conversation = self.conversation(
            &inbound.adapter_id,
            &inbound.peer_id,
            &inbound.agent_id,
            inbound.title.as_deref().unwrap_or(&inbound.peer_id),
        )?;
        let message = self.insert_message(
            &conversation.id,
            Direction::Human,
            &inbound.text,
            &inbound.kind,
            inbound.payload.as_ref(),
            Some(mailbox_id),
            Some(&inbound.external_id),
            inbound.reply_to_external_id.as_deref(),
            DeliveryStatus::Received,
            now(),
        )?;
        self.link_mailbox(
            mailbox_id,
            &conversation,
            message.id,
            Some(&inbound.external_id),
        )?;
        self.conn.execute("INSERT INTO chat_sources(adapter_id, peer_id, external_id, fingerprint, message_id) VALUES (?1, ?2, ?3, ?4, ?5)", params![inbound.adapter_id, inbound.peer_id, inbound.external_id, source_fingerprint(inbound), message.id])?;
        Ok(message)
    }

    fn source(&self, inbound: &InboundMessage) -> Result<Option<ChatMessage>> {
        let stored: Option<(String, i64)> = self.conn.query_row("SELECT fingerprint, message_id FROM chat_sources WHERE adapter_id = ?1 AND peer_id = ?2 AND external_id = ?3", params![inbound.adapter_id, inbound.peer_id, inbound.external_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        match stored {
            Some((fingerprint, id)) if fingerprint == source_fingerprint(inbound) => {
                Ok(Some(self.message(id)?))
            }
            Some(_) => Err(Error::IdempotencyConflict),
            None => Ok(None),
        }
    }

    fn existing_inbound(&self, id: i64) -> Result<ExistingInbound> {
        self.conn.query_row("SELECT sender, recipient, text, COALESCE(kind, 'text'), structured_payload, conversation_id, chat_message_id, (delivered_at IS NOT NULL OR acked_at IS NOT NULL) FROM messages WHERE id = ?1", [id], |row| Ok(ExistingInbound { sender: row.get(0)?, recipient: row.get(1)?, text: row.get(2)?, kind: row.get(3)?, payload: row.get(4)?, conversation_id: row.get(5)?, chat_id: row.get(6)?, consumed: row.get(7)? })).optional()?.ok_or_else(|| Error::NotFound("inbound mailbox message".into()))
    }
}

struct ExistingInbound {
    sender: String,
    recipient: String,
    text: String,
    kind: String,
    payload: Option<String>,
    conversation_id: Option<String>,
    chat_id: Option<i64>,
    consumed: bool,
}

impl ExistingInbound {
    fn validate(&self, inbound: &InboundMessage) -> Result<()> {
        if !self.sender.starts_with("user:")
            || self.recipient != inbound.agent_id
            || self.kind == "media_pending"
        {
            return Err(Error::Forbidden);
        }
        let payload: Option<Value> = self
            .payload
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        if self.text != inbound.text || self.kind != inbound.kind || payload != inbound.payload {
            return Err(Error::IdempotencyConflict);
        }
        Ok(())
    }
}

fn validate(inbound: &InboundMessage) -> Result<()> {
    for (label, value) in [
        ("adapter ID", &inbound.adapter_id),
        ("peer ID", &inbound.peer_id),
        ("sender ID", &inbound.sender_id),
        ("agent ID", &inbound.agent_id),
        ("external ID", &inbound.external_id),
    ] {
        identity(label, value)?;
    }
    if inbound.adapter_id == "local" {
        return Err(Error::Forbidden);
    }
    if matches!(inbound.kind.as_str(), "typing" | "reaction") {
        return Err(Error::InvalidInput(
            "transport control kinds cannot enter an agent inbox".into(),
        ));
    }
    if let Some(id) = &inbound.reply_to_external_id {
        identity("reply external ID", id)?;
    }
    if let Some(title) = &inbound.title {
        identity("conversation title", title)?;
    }
    content(&inbound.text, &inbound.kind, &inbound.payload)
}

fn source_fingerprint(inbound: &InboundMessage) -> String {
    // The configured agent and mutable display title are deliberately omitted:
    // replay after rerouting must return the original conversation.
    fingerprint(&serde_json::json!([
        inbound.sender_id,
        inbound.text,
        inbound.kind,
        inbound.payload,
        inbound.reply_to_external_id
    ]))
}
