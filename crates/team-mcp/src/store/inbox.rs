use anyhow::Result;
use rusqlite::{params, Connection};

use super::{Message, Store};

fn acknowledge(conn: &Connection, agent_id: &str, id: i64, now: f64) -> Result<usize> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO message_receipts (message_id, agent_id, acked_at)
         SELECT m.id, ?1, ?3 FROM messages m
         WHERE m.id = ?2 AND m.acked_at IS NULL AND m.sender != ?1
           AND (m.kind IS NULL OR m.kind != 'media_pending')
           AND (m.recipient = ?1 OR m.recipient IN (
               SELECT 'channel:' || channel_id FROM channel_members WHERE agent_id = ?1
           ))",
        params![agent_id, id, now],
    )?;
    if inserted > 0 {
        // Preserve global acknowledgement for old readers and garbage
        // collection, but only after every current subscriber has consumed.
        conn.execute(
            "UPDATE messages SET acked_at = ?3 WHERE id = ?2 AND acked_at IS NULL
             AND (recipient = ?1 OR NOT EXISTS (
                 SELECT 1 FROM channel_members cm
                 WHERE 'channel:' || cm.channel_id = messages.recipient
                   AND cm.agent_id != messages.sender
                   AND NOT EXISTS (SELECT 1 FROM message_receipts mr
                       WHERE mr.message_id = messages.id AND mr.agent_id = cm.agent_id)
             ))",
            params![agent_id, id, now],
        )?;
    }
    Ok(inserted)
}

impl Store {
    /// Peek undelivered/unacked messages addressed directly to `agent_id` or
    /// to a channel `agent_id` subscribes to.
    pub fn inbox_peek(&self, agent_id: &str, limit: usize) -> Result<Vec<Message>> {
        self.inbox_since(agent_id, 0, limit)
    }

    pub fn inbox_high_water(&self, agent_id: &str) -> Result<i64> {
        let conn = self.connection()?;
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE recipient = ?1 OR recipient IN (
                 SELECT 'channel:' || channel_id FROM channel_members WHERE agent_id = ?1
             )",
            params![agent_id],
            |row| row.get(0),
        )?)
    }

    /// Select new visible messages before applying the batch limit. Watchers
    /// must not let an older unconsumed backlog hide later notifications.
    pub fn inbox_since(&self, agent_id: &str, after_id: i64, limit: usize) -> Result<Vec<Message>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.project_id, m.sender, m.recipient, m.text, m.thread_id, m.sent_at,
                    m.telegram_msg_id, m.kind, m.structured_payload, m.delivery_mode,
                    m.conversation_id, m.chat_message_id, m.adapter_id, m.external_id, m.peer_id, m.interface_json
             FROM messages m
             WHERE m.acked_at IS NULL
               AND m.id > ?3
               AND (m.kind IS NULL OR m.kind != 'media_pending')
               AND NOT EXISTS (
                   SELECT 1 FROM message_receipts mr
                   WHERE mr.message_id = m.id AND mr.agent_id = ?1
               )
               AND m.sender != ?1
               AND (
                     m.recipient = ?1
                  OR m.recipient IN (
                        SELECT 'channel:' || cm.channel_id
                        FROM channel_members cm
                        WHERE cm.agent_id = ?1
                     )
                 )
             ORDER BY m.id ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![agent_id, limit as i64, after_id], |r| {
                Ok(Message {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    sender: r.get(2)?,
                    recipient: r.get(3)?,
                    text: r.get(4)?,
                    thread_id: r.get(5)?,
                    sent_at: r.get(6)?,
                    telegram_msg_id: r.get(7)?,
                    kind: r.get(8)?,
                    structured_payload: r.get(9)?,
                    delivery_mode: r.get(10)?,
                    conversation_id: r.get(11)?,
                    chat_message_id: r.get(12)?,
                    adapter_id: r.get(13)?,
                    external_id: r.get(14)?,
                    peer_id: r.get(15)?,
                    interface: r
                        .get::<_, Option<String>>(16)?
                        .map(|s| serde_json::from_str(&s))
                        .transpose()
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                16,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Fetch full bodies for the listed ids (filtered to messages the caller
    /// is allowed to see — i.e. addressed to `agent_id` directly or to a
    /// channel they subscribe to) and auto-ack them in the same transaction
    /// (T-104). Returns the rows in the order requested. Ids the caller
    /// can't see, or already-acked ids, are silently skipped — the caller's
    /// response is whatever they were entitled to read.
    pub fn inbox_read(&self, agent_id: &str, ids: &[i64]) -> Result<Vec<Message>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        // Pull rows the caller is allowed to read AND that are still unacked.
        let select_sql = format!(
            "SELECT m.id, m.project_id, m.sender, m.recipient, m.text, m.thread_id, m.sent_at,
                    m.telegram_msg_id, m.kind, m.structured_payload, m.delivery_mode,
                    m.conversation_id, m.chat_message_id, m.adapter_id, m.external_id, m.peer_id, m.interface_json
             FROM messages m
             WHERE m.id IN ({placeholders})
               AND m.acked_at IS NULL
               AND (m.kind IS NULL OR m.kind != 'media_pending')
               AND NOT EXISTS (
                   SELECT 1 FROM message_receipts mr
                   WHERE mr.message_id = m.id AND mr.agent_id = ?
               )
               AND m.sender != ?
               AND (
                     m.recipient = ?
                  OR m.recipient IN (
                        SELECT 'channel:' || cm.channel_id
                        FROM channel_members cm
                        WHERE cm.agent_id = ?
                     )
                 )"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 4);
        for id in ids {
            params_vec.push(Box::new(*id));
        }
        params_vec.push(Box::new(agent_id.to_string()));
        params_vec.push(Box::new(agent_id.to_string()));
        params_vec.push(Box::new(agent_id.to_string()));
        params_vec.push(Box::new(agent_id.to_string()));
        let refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| &**b).collect();
        let rows: Vec<Message> = {
            let mut stmt = tx.prepare(&select_sql)?;
            let collected = stmt
                .query_map(refs.as_slice(), |r| {
                    Ok(Message {
                        id: r.get(0)?,
                        project_id: r.get(1)?,
                        sender: r.get(2)?,
                        recipient: r.get(3)?,
                        text: r.get(4)?,
                        thread_id: r.get(5)?,
                        sent_at: r.get(6)?,
                        telegram_msg_id: r.get(7)?,
                        kind: r.get(8)?,
                        structured_payload: r.get(9)?,
                        delivery_mode: r.get(10)?,
                        conversation_id: r.get(11)?,
                        chat_message_id: r.get(12)?,
                        adapter_id: r.get(13)?,
                        external_id: r.get(14)?,
                        peer_id: r.get(15)?,
                        interface: r
                            .get::<_, Option<String>>(16)?
                            .map(|s| serde_json::from_str(&s))
                            .transpose()
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    16,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            collected
        };
        for row in &rows {
            acknowledge(&tx, agent_id, row.id, Self::now())?;
        }
        tx.commit()?;
        // Preserve caller-supplied id order for predictability.
        let mut by_id: std::collections::HashMap<i64, Message> =
            rows.into_iter().map(|m| (m.id, m)).collect();
        let ordered = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        Ok(ordered)
    }

    /// Consume only messages visible to this caller. Channel consumption is
    /// recorded independently for each subscriber.
    pub fn inbox_ack(&self, agent_id: &str, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let mut n = 0;
        for id in ids {
            n += acknowledge(&tx, agent_id, *id, Self::now())?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Unacked count for an agent. Used by `teamctl status`.
    #[allow(dead_code)]
    pub fn inbox_depth(&self, agent_id: &str) -> Result<i64> {
        let conn = self.connection()?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages m WHERE m.acked_at IS NULL
             AND m.sender != ?1 AND (m.kind IS NULL OR m.kind != 'media_pending')
             AND NOT EXISTS (SELECT 1 FROM message_receipts mr
                 WHERE mr.message_id=m.id AND mr.agent_id=?1)
             AND (m.recipient=?1 OR m.recipient IN (
                 SELECT 'channel:' || channel_id FROM channel_members WHERE agent_id=?1
             ))",
            params![agent_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }
}
