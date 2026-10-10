use anyhow::{bail, Result};
use rusqlite::{params, Connection};

use super::Store;

/// Resolve a mailbox id to the `telegram_msg_id` of that row, if any.
///
/// T-168: agents pass `reply_to_message_id` as a mailbox id (the value
/// they have from the channel envelope's `meta.id`). The bot's outbound
/// dispatcher actually needs the Telegram message id, which lives on the
/// referenced row. Resolving at insert time pins the resolved value on
/// the outgoing row — no extra DB roundtrip on the bot's hot dispatch
/// path, and the value can't drift if the source row is later mutated.
///
/// `None` input (no threading requested) returns `None` without a
/// lookup. `Some(id)` that doesn't resolve (row missing, or row has no
/// `telegram_msg_id`) is warn-logged and returns `None` — the outgoing
/// message still sends, just without `reply_parameters`. We never fail
/// the insert over a bad threading hint.
fn resolve_telegram_msg_id(conn: &Connection, mailbox_id: Option<i64>) -> Option<i64> {
    let mid = mailbox_id?;
    match conn.query_row(
        "SELECT telegram_msg_id FROM messages WHERE id = ?1",
        params![mid],
        |r| r.get::<_, Option<i64>>(0),
    ) {
        Ok(Some(tid)) => Some(tid),
        Ok(None) => {
            tracing::warn!(
                mailbox_id = mid,
                "reply_to_message_id: referenced mailbox row has no telegram_msg_id; reply will land without threading"
            );
            None
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            tracing::warn!(
                mailbox_id = mid,
                "reply_to_message_id: mailbox row not found; reply will land without threading"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                mailbox_id = mid,
                error = %e,
                "reply_to_message_id: lookup failed; reply will land without threading"
            );
            None
        }
    }
}

impl Store {
    /// Insert a DM (recipient is `<project>:<agent>`). `reply_to_mailbox_id`
    /// (T-168) is the **mailbox id** of the inbound user message this row
    /// replies to — the obvious value an agent has from the channel
    /// envelope. We resolve it to the row's `telegram_msg_id` at insert
    /// time and persist that on the outgoing row, so the bot's outbound
    /// dispatcher reads a value it can hand to `reply_parameters` without
    /// a second roundtrip. Resolution misses warn-log and leave the column
    /// NULL (message still sends, just without threading). `None` skips
    /// resolution entirely — peer-to-peer DMs and non-Telegram paths take
    /// this branch.
    #[allow(clippy::too_many_arguments)]
    pub fn send_dm(
        &self,
        project: &str,
        sender: &str,
        recipient: &str,
        text: &str,
        thread_id: Option<&str>,
        reply_to_mailbox_id: Option<i64>,
    ) -> Result<i64> {
        let conn = self.connection()?;
        let telegram_msg_id = resolve_telegram_msg_id(&conn, reply_to_mailbox_id);
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, thread_id, sent_at, telegram_msg_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project,
                sender,
                recipient,
                text,
                thread_id,
                Self::now(),
                telegram_msg_id,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a structured-content DM (image/file/etc.). `kind` is the
    /// discriminator the bot's outbound dispatcher matches on; `payload` is
    /// the JSON-encoded content descriptor (e.g. `{"source":"path",
    /// "value":"/tmp/x.png","caption":"…"}`). The `text` column carries the
    /// caption (if any) so legacy text-only readers still see something
    /// meaningful; the structured payload is the source of truth for
    /// dispatch. `reply_to_mailbox_id` plumbs Telegram threading the same
    /// way it does for `send_dm` — see that fn's doc for the resolution
    /// contract.
    #[allow(clippy::too_many_arguments)]
    pub fn send_dm_kind(
        &self,
        project: &str,
        sender: &str,
        recipient: &str,
        text: &str,
        thread_id: Option<&str>,
        kind: &str,
        payload: &str,
        reply_to_mailbox_id: Option<i64>,
    ) -> Result<i64> {
        // #254: `system` is a privileged kind — lifecycle signals (drain,
        // startup, rate-limit) the supervisor emits, delivered inline +
        // real-time. Only `system:*` sources may originate it; if any
        // agent or `user:*` could, a forged "session terminating" signal
        // would be trivial. This is the single insert choke point for
        // non-text kinds, so the allowlist lives here. The privileged-kind
        // predicate is shared with the UPDATE guard (#320) so both write
        // paths agree on what "privileged" means.
        if team_core::mailbox::is_privileged_kind(kind) && !sender.starts_with("system:") {
            bail!("kind 'system' may only be sent by a system:* source (got sender '{sender}')");
        }
        let conn = self.connection()?;
        let telegram_msg_id = resolve_telegram_msg_id(&conn, reply_to_mailbox_id);
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, thread_id, sent_at,
                 kind, structured_payload, telegram_msg_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                project,
                sender,
                recipient,
                text,
                thread_id,
                Self::now(),
                kind,
                payload,
                telegram_msg_id,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a broadcast message addressed to `channel:<project>:<name>`.
    pub fn send_broadcast(
        &self,
        project: &str,
        sender: &str,
        channel_name: &str,
        text: &str,
    ) -> Result<i64> {
        let conn = self.connection()?;
        let recipient = format!("channel:{project}:{channel_name}");
        conn.execute(
            "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project, sender, recipient, text, Self::now()],
        )?;
        Ok(conn.last_insert_rowid())
    }
}
