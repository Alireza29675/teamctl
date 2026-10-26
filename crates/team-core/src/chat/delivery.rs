use rusqlite::{params, OptionalExtension};

use super::repository::{identity, now, page_size};
use super::{Delivery, Error, Repository, Result};

const MAX_ATTEMPTS: u32 = 10;

impl Repository<'_> {
    pub fn claim_deliveries(
        &self,
        adapter_id: &str,
        owner: &str,
        now: f64,
        lease_seconds: f64,
        limit: usize,
    ) -> Result<Vec<Delivery>> {
        identity("adapter ID", adapter_id)?;
        identity("lease owner", owner)?;
        if !now.is_finite()
            || now < 0.0
            || !lease_seconds.is_finite()
            || !(0.0..=3600.0).contains(&lease_seconds)
            || lease_seconds == 0.0
        {
            return Err(Error::InvalidInput(
                "invalid delivery clock or lease length".into(),
            ));
        }
        let limit = page_size(limit)?;
        self.atomic(|| {
            let mut stmt = self.conn.prepare("SELECT id FROM chat_deliveries WHERE adapter_id = ?1 AND finished = 0 AND next_attempt_at <= ?2 AND (lease_until IS NULL OR lease_until <= ?2) ORDER BY id LIMIT ?3")?;
            let ids = stmt.query_map(params![adapter_id, now, limit], |row| row.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut deliveries = Vec::with_capacity(ids.len());
            for id in ids {
                let (message_id, attempts): (i64, u32) = self.conn.query_row("SELECT message_id, attempts FROM chat_deliveries WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))?;
                // A crashing adapter cannot lease a poison delivery forever.
                if attempts >= MAX_ATTEMPTS {
                    self.conn.execute("UPDATE chat_deliveries SET finished = 1, lease_owner = NULL, lease_until = NULL WHERE id = ?1", [id])?;
                    self.update_status(message_id, "failed", Some("delivery retry limit reached"), None, now)?;
                    continue;
                }
                self.conn.execute("UPDATE chat_deliveries SET lease_owner = ?1, lease_until = ?2, attempts = attempts + 1 WHERE id = ?3", params![owner, now + lease_seconds, id])?;
                self.update_status(message_id, "queued", None, None, now)?;
                let message = self.message(message_id)?;
                let conversation = self.get_conversation(&message.conversation_id)?;
                let quote = self.conn.query_row("SELECT reply_to_external_id FROM chat_messages WHERE id = ?1", [message_id], |row| row.get(0))?;
                deliveries.push(Delivery { id, conversation_id: conversation.id, adapter_id: conversation.adapter_id, peer_id: conversation.peer_id, message, reply_to_external_id: quote, attempts: attempts + 1 });
            }
            Ok(deliveries)
        })
    }

    /// Confirm transport acceptance, not the recipient's read status.
    pub fn mark_sent(
        &self,
        delivery_id: i64,
        owner: &str,
        external_id: Option<&str>,
    ) -> Result<()> {
        identity("lease owner", owner)?;
        if let Some(id) = external_id {
            identity("external ID", id)?;
        }
        self.atomic(|| {
            let now = now();
            let (message_id, _) = self.owned_delivery(delivery_id, owner, now)?;
            self.finish(delivery_id)?;
            self.update_status(message_id, "sent", None, external_id, now)
        })
    }

    pub fn mark_failed(
        &self,
        delivery_id: i64,
        owner: &str,
        reason: &str,
        retryable: bool,
    ) -> Result<()> {
        identity("lease owner", owner)?;
        if reason.is_empty() || reason.len() > 4096 || reason.contains('\0') {
            return Err(Error::InvalidInput(
                "delivery error must be nonempty and bounded".into(),
            ));
        }
        self.atomic(|| {
            let now = now();
            let (message_id, attempts) = self.owned_delivery(delivery_id, owner, now)?;
            if retryable && attempts < MAX_ATTEMPTS {
                let delay = (2_u32.pow(attempts.min(8))).min(300) as f64;
                self.conn.execute("UPDATE chat_deliveries SET lease_owner = NULL, lease_until = NULL, next_attempt_at = ?1 WHERE id = ?2", params![now + delay, delivery_id])?;
            } else { self.finish(delivery_id)?; }
            self.update_status(message_id, "failed", Some(reason), None, now)
        })
    }

    fn owned_delivery(&self, id: i64, owner: &str, now: f64) -> Result<(i64, u32)> {
        self.conn.query_row("SELECT message_id, attempts FROM chat_deliveries WHERE id = ?1 AND finished = 0 AND lease_owner = ?2 AND lease_until > ?3", params![id, owner, now], |row| Ok((row.get(0)?, row.get(1)?))).optional()?.ok_or(Error::LeaseLost)
    }

    fn finish(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE chat_deliveries SET finished = 1, lease_owner = NULL, lease_until = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    fn update_status(
        &self,
        message: i64,
        status: &str,
        error: Option<&str>,
        external: Option<&str>,
        now: f64,
    ) -> Result<()> {
        self.conn.execute("UPDATE chat_messages SET status = ?1, error = ?2, external_id = COALESCE(?3, external_id), updated_at = ?4, revision = revision + 1 WHERE id = ?5", params![status, error, external, now, message])?;
        self.touch_conversation(&self.message(message)?.conversation_id, now)
    }
}
