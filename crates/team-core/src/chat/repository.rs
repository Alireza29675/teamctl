use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;

use super::{
    ChatMessage, Conversation, DeliveryStatus, Direction, Error, Result, MAX_ID_BYTES,
    MAX_PAGE_SIZE, MAX_TEXT_BYTES,
};

pub(super) const MESSAGE_COLUMNS: &str = "id, conversation_id, direction, text, kind, payload_json, mailbox_id, external_id, status, error, created_at, updated_at, revision";

pub struct Repository<'a> {
    pub(super) conn: &'a Connection,
}

struct InboundRoute {
    recipient: String,
    sender: String,
    conversation_id: Option<String>,
    chat_message_id: Option<i64>,
    kind: Option<String>,
}

impl<'a> Repository<'a> {
    /// Transport capability declarations are data, independent of provider code.
    pub fn register_capabilities(&self, adapter: &str, capabilities: &[String]) -> Result<()> {
        identity("adapter ID", adapter)?;
        if capabilities.len() > 32 {
            return Err(Error::InvalidInput("too many adapter capabilities".into()));
        }
        for capability in capabilities {
            identity("capability", capability)?;
        }
        self.conn.execute("INSERT INTO chat_adapter_capabilities(adapter_id,capabilities_json) VALUES(?1,?2) ON CONFLICT(adapter_id) DO UPDATE SET capabilities_json=excluded.capabilities_json",params![adapter,serde_json::to_string(capabilities)?])?;
        Ok(())
    }

    pub fn capabilities(&self, adapter: &str) -> Result<Option<Vec<String>>> {
        if adapter == "local" {
            return Ok(Some(vec!["text".into()]));
        }
        let encoded: Option<String> = self
            .conn
            .query_row(
                "SELECT capabilities_json FROM chat_adapter_capabilities WHERE adapter_id=?1",
                [adapter],
                |r| r.get(0),
            )
            .optional()?;
        encoded
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn open_local(&self, agent_id: &str) -> Result<Conversation> {
        self.atomic(|| {
            self.require_agent(agent_id)?;
            self.conversation("local", "operator", agent_id, agent_id)
        })
    }

    pub fn get_conversation(&self, id: &str) -> Result<Conversation> {
        self.conn.query_row(
            "SELECT id, adapter_id, peer_id, agent_id, title, created_at, updated_at, revision FROM chat_conversations WHERE id = ?1",
            [id], conversation_row,
        ).optional()?.ok_or_else(|| Error::NotFound("conversation".into()))
    }

    pub fn list_conversations(&self, limit: usize) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, adapter_id, peer_id, agent_id, title, created_at, updated_at, revision FROM chat_conversations ORDER BY updated_at DESC, id LIMIT ?1",
        )?;
        let rows = stmt.query_map([page_size(limit)?], conversation_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The newest page is returned in chronological order, as are older pages.
    /// `after` pages advance from the requested ID instead of skipping a backlog.
    pub fn history(
        &self,
        id: &str,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ChatMessage>> {
        if before.is_some() && after.is_some() {
            return Err(Error::InvalidInput(
                "before and after cannot be combined".into(),
            ));
        }
        self.get_conversation(id)?;
        let limit = page_size(limit)?;
        let order = if after.is_some() { "ASC" } else { "DESC" };
        let sql = format!("SELECT {MESSAGE_COLUMNS} FROM chat_messages WHERE conversation_id = ?1 AND (?2 IS NULL OR id < ?2) AND (?3 IS NULL OR id > ?3) ORDER BY id {order} LIMIT ?4");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut messages = stmt
            .query_map(params![id, before, after, limit], message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if after.is_none() {
            messages.reverse();
        }
        Ok(messages)
    }

    pub fn send_local(
        &self,
        conversation_id: &str,
        text: &str,
        request_id: &str,
    ) -> Result<ChatMessage> {
        identity("request ID", request_id)?;
        content(text, "text", &None)?;
        self.atomic(|| {
            let conversation = self.get_conversation(conversation_id)?;
            if conversation.adapter_id != "local" || conversation.peer_id != "operator" {
                return Err(Error::Forbidden);
            }
            let fingerprint = fingerprint(&serde_json::json!([conversation_id, text]));
            if let Some(message) = self.request("local:operator", request_id, &fingerprint)? {
                return Ok(message);
            }
            let project = self.require_agent(&conversation.agent_id)?;
            let now = now();
            self.conn.execute(
                "INSERT INTO messages(project_id, sender, recipient, text, kind, sent_at) VALUES (?1, 'user:local', ?2, ?3, 'text', ?4)",
                params![project, conversation.agent_id, text, now],
            )?;
            let mailbox_id = self.conn.last_insert_rowid();
            let message = self.insert_message(&conversation.id, Direction::Human, text, "text", None, Some(mailbox_id), None, None, DeliveryStatus::Received, now)?;
            self.link_mailbox(mailbox_id, &conversation, message.id, None)?;
            self.save_request("local:operator", request_id, &fingerprint, message.id)?;
            Ok(message)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reply(
        &self,
        agent_id: &str,
        conversation_id: Option<&str>,
        reply_to_mailbox_id: Option<i64>,
        text: &str,
        kind: &str,
        payload: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<ChatMessage> {
        content(text, kind, &payload)?;
        if let Some(id) = request_id {
            identity("request ID", id)?;
        }
        self.atomic(|| {
            self.require_agent(agent_id)?;
            let caller = format!("agent:{agent_id}");
            // Fingerprint caller input, before resolving disposable inbox IDs.
            // A successful retry remains recognizable after inbox GC or after
            // another human opens a second route to the same agent.
            let fingerprint = fingerprint(&serde_json::json!([conversation_id, reply_to_mailbox_id, text, kind, payload]));
            if let Some(id) = request_id {
                if let Some(message) = self.request(&caller, id, &fingerprint)? { return Ok(message); }
            }
            let (conversation, quoted_external_id) = self.resolve_route(agent_id, conversation_id, reply_to_mailbox_id)?;
            let now = now();
            let status = if conversation.adapter_id == "local" { DeliveryStatus::Sent } else { DeliveryStatus::Queued };
            let message = self.insert_message(&conversation.id, Direction::Agent, text, kind, payload.as_ref(), None, None, quoted_external_id.as_deref(), status, now)?;
            if status == DeliveryStatus::Queued {
                self.conn.execute("INSERT INTO chat_deliveries(message_id, adapter_id, next_attempt_at) VALUES (?1, ?2, ?3)", params![message.id, conversation.adapter_id, now])?;
            }
            if let Some(id) = request_id { self.save_request(&caller, id, &fingerprint, message.id)?; }
            Ok(message)
        })
    }

    /// Resolve an authorized destination without enqueueing a reply.
    /// Callers must keep any dependent validation and writes in one transaction.
    pub fn resolve_route(
        &self,
        agent: &str,
        explicit: Option<&str>,
        mailbox_id: Option<i64>,
    ) -> Result<(Conversation, Option<String>)> {
        let referenced = if let Some(id) = mailbox_id {
            let row = self.conn.query_row(
                "SELECT recipient, sender, conversation_id, chat_message_id, kind FROM messages WHERE id = ?1", [id],
                |row| Ok(InboundRoute { recipient: row.get(0)?, sender: row.get(1)?, conversation_id: row.get(2)?, chat_message_id: row.get(3)?, kind: row.get(4)? }),
            ).optional()?;
            let row = row.ok_or_else(|| Error::NotFound("inbound mailbox message".into()))?;
            if row.recipient != agent
                || !row.sender.starts_with("user:")
                || row.kind.as_deref() == Some("media_pending")
            {
                return Err(Error::Forbidden);
            }
            let route = row.conversation_id.ok_or(Error::NoRoute)?;
            let message = self.message(row.chat_message_id.ok_or(Error::NoRoute)?)?;
            if message.direction != Direction::Human
                || message.conversation_id != route
                || message.mailbox_id != Some(id)
            {
                return Err(Error::Forbidden);
            }
            Some((self.get_conversation(&route)?, message.external_id))
        } else {
            None
        };
        if let Some(id) = explicit {
            let conversation = self.get_conversation(id)?;
            if conversation.agent_id != agent {
                return Err(Error::Forbidden);
            }
            if let Some((route, quote)) = referenced {
                if route.id != conversation.id {
                    return Err(Error::Forbidden);
                }
                return Ok((conversation, quote));
            }
            return Ok((conversation, None));
        }
        if let Some((conversation, quote)) = referenced {
            if conversation.agent_id != agent {
                return Err(Error::Forbidden);
            }
            return Ok((conversation, quote));
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM chat_conversations WHERE agent_id = ?1 LIMIT 2")?;
        let ids = stmt
            .query_map([agent], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids.as_slice() {
            [] => Err(Error::NoRoute),
            [id] => Ok((self.get_conversation(id)?, None)),
            _ => Err(Error::AmbiguousRoute),
        }
    }

    pub(super) fn require_agent(&self, id: &str) -> Result<String> {
        identity("agent ID", id)?;
        self.conn
            .query_row("SELECT project_id FROM agents WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or_else(|| Error::UnknownAgent(id.into()))
    }

    pub(super) fn conversation(
        &self,
        adapter: &str,
        peer: &str,
        agent: &str,
        title: &str,
    ) -> Result<Conversation> {
        let id = format!(
            "chat:{}",
            fingerprint(&serde_json::json!([adapter, peer, agent]))
        );
        let now = now();
        self.conn.execute("INSERT OR IGNORE INTO chat_conversations(id, adapter_id, peer_id, agent_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)", params![id, adapter, peer, agent, title, now])?;
        self.get_conversation(&id)
    }

    pub(super) fn message(&self, id: i64) -> Result<ChatMessage> {
        self.conn
            .query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM chat_messages WHERE id = ?1"),
                [id],
                message_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound("chat message".into()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn insert_message(
        &self,
        conversation: &str,
        direction: Direction,
        text: &str,
        kind: &str,
        payload: Option<&Value>,
        mailbox: Option<i64>,
        external: Option<&str>,
        quote: Option<&str>,
        status: DeliveryStatus,
        now: f64,
    ) -> Result<ChatMessage> {
        self.conn.execute("INSERT INTO chat_messages(conversation_id, direction, text, kind, payload_json, mailbox_id, external_id, reply_to_external_id, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![conversation, if direction == Direction::Human { "human" } else { "agent" }, text, kind, payload.map(serde_json::to_string).transpose()?, mailbox, external, quote, status_name(status), now])?;
        let id = self.conn.last_insert_rowid();
        self.touch_conversation(conversation, now)?;
        self.message(id)
    }

    pub(super) fn touch_conversation(&self, id: &str, now: f64) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_conversations SET updated_at = ?1, revision = revision + 1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub(super) fn link_mailbox(
        &self,
        mailbox: i64,
        conversation: &Conversation,
        chat: i64,
        external: Option<&str>,
    ) -> Result<()> {
        self.conn.execute("UPDATE messages SET conversation_id = ?1, chat_message_id = ?2, adapter_id = ?3, external_id = ?4, peer_id = ?5 WHERE id = ?6", params![conversation.id, chat, conversation.adapter_id, external, conversation.peer_id, mailbox])?;
        Ok(())
    }

    fn request(&self, caller: &str, id: &str, fingerprint: &str) -> Result<Option<ChatMessage>> {
        let row: Option<(String, i64)> = self.conn.query_row("SELECT fingerprint, message_id FROM chat_requests WHERE caller = ?1 AND request_id = ?2", params![caller, id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        match row {
            Some((stored, message)) if stored == fingerprint => Ok(Some(self.message(message)?)),
            Some(_) => Err(Error::IdempotencyConflict),
            None => Ok(None),
        }
    }

    fn save_request(&self, caller: &str, id: &str, fingerprint: &str, message: i64) -> Result<()> {
        self.conn.execute("INSERT INTO chat_requests(caller, request_id, fingerprint, message_id) VALUES (?1, ?2, ?3, ?4)", params![caller, id, fingerprint, message])?;
        Ok(())
    }

    /// Acquire the write reservation before reading deduplication/lease state.
    /// Inside a native adapter's transaction, use a savepoint without committing
    /// its surrounding media-publication operation.
    pub(super) fn atomic<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let outer = self.conn.is_autocommit();
        self.conn.execute_batch(if outer {
            "BEGIN IMMEDIATE"
        } else {
            "SAVEPOINT team_chat_operation"
        })?;
        match operation() {
            Ok(value) => {
                if let Err(error) = self.conn.execute_batch(if outer {
                    "COMMIT"
                } else {
                    "RELEASE team_chat_operation"
                }) {
                    self.conn.execute_batch(if outer {
                        "ROLLBACK"
                    } else {
                        "ROLLBACK TO team_chat_operation; RELEASE team_chat_operation"
                    })?;
                    return Err(error.into());
                }
                Ok(value)
            }
            Err(error) => {
                self.conn.execute_batch(if outer {
                    "ROLLBACK"
                } else {
                    "ROLLBACK TO team_chat_operation; RELEASE team_chat_operation"
                })?;
                Err(error)
            }
        }
    }
}

pub(super) fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub(super) fn fingerprint(value: &Value) -> String {
    blake3::hash(value.to_string().as_bytes())
        .to_hex()
        .to_string()
}

pub(super) fn page_size(limit: usize) -> Result<i64> {
    if limit == 0 {
        return Err(Error::InvalidInput("page size must be positive".into()));
    }
    Ok(limit.min(MAX_PAGE_SIZE) as i64)
}

pub(super) fn identity(label: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(format!(
            "{label} must be nonempty, bounded, and contain no control characters"
        )));
    }
    Ok(())
}

pub(super) fn content(text: &str, kind: &str, payload: &Option<Value>) -> Result<()> {
    if text.len() > MAX_TEXT_BYTES || text.contains('\0') || (text.is_empty() && kind == "text") {
        return Err(Error::InvalidInput(
            "text is empty or exceeds the text limit".into(),
        ));
    }
    if !matches!(
        kind,
        "text"
            | "image"
            | "file"
            | "audio"
            | "video"
            | "voice"
            | "media"
            | "reaction"
            | "typing"
            | "media_error"
    ) {
        return Err(Error::InvalidInput(
            "unsupported or privileged message kind".into(),
        ));
    }
    if payload
        .as_ref()
        .is_some_and(|v| v.to_string().len() > super::protocol::MAX_FRAME_BYTES / 2)
    {
        return Err(Error::InvalidInput(
            "structured payload exceeds its limit".into(),
        ));
    }
    Ok(())
}

pub(super) fn status_name(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Queued => "queued",
        DeliveryStatus::Sent => "sent",
        DeliveryStatus::Failed => "failed",
        DeliveryStatus::Received => "received",
    }
}

fn conversation_row(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        adapter_id: row.get(1)?,
        peer_id: row.get(2)?,
        agent_id: row.get(3)?,
        title: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        revision: row.get(7)?,
    })
}

pub(super) fn message_row(row: &Row<'_>) -> rusqlite::Result<ChatMessage> {
    let direction: String = row.get(2)?;
    let status: String = row.get(8)?;
    let payload: Option<String> = row.get(5)?;
    Ok(ChatMessage {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        direction: if direction == "human" {
            Direction::Human
        } else {
            Direction::Agent
        },
        text: row.get(3)?,
        kind: row.get(4)?,
        payload: payload
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        mailbox_id: row.get(6)?,
        external_id: row.get(7)?,
        status: match status.as_str() {
            "queued" => DeliveryStatus::Queued,
            "sent" => DeliveryStatus::Sent,
            "failed" => DeliveryStatus::Failed,
            _ => DeliveryStatus::Received,
        },
        error: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        revision: row.get(12)?,
    })
}
