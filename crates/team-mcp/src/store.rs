//! SQLite-backed message store. One connection per process.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

pub struct Store {
    pub(crate) conn: Mutex<Connection>,
}

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

fn try_open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).context("open sqlite")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    team_core::mailbox::ensure(&conn)?;
    Ok(conn)
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub project_id: String,
    pub sender: String,
    pub recipient: String,
    pub text: String,
    pub thread_id: Option<String>,
    pub sent_at: f64,
    /// Telegram message id this row pertains to. On inbound rows
    /// (sender = `user:telegram`) it's the source message the operator
    /// typed — agents read this to populate `react_to_user.telegram_msg_id`
    /// for emoji reactions. (For reply threading, agents pass the row's
    /// mailbox id as `reply_to_user.reply_to_message_id`; T-168 resolves
    /// it to this column at insert time.) On outbound rows (sender =
    /// `<project>:<agent>`) it's the resolved id `reply_parameters` should
    /// target. `None` on rows unrelated to Telegram.
    pub telegram_msg_id: Option<i64>,
    /// Mailbox-kind discriminator (T-086-A schema). `None` for legacy
    /// text-only rows; `Some("image"|"file"|"media_error"|...)` for
    /// structured rows the agent should handle differently from plain text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// JSON-encoded content descriptor for non-text rows. Shape depends on
    /// `kind`: image/file rows carry `{path, caption?, mime?, size_bytes?}`;
    /// media_error rows carry `{error, caption?}`. Agents parse this on
    /// demand — text-only callers see `None` and ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_payload: Option<String>,
    /// Per-message delivery mode (T-104 lazy inbox). `None` (the default)
    /// means the channel watcher emits a stub notification; the agent drills
    /// in via `inbox_read`. `Some("immediate")` means the full body is
    /// delivered inline, bypassing the stub — set by the bot when an
    /// operator prefixes a message with `/readnow `.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<String>,
}

/// One approval row's decision-relevant fields. `value` is the chosen
/// option's `value` for a multi-option decision (#299); `None` for the
/// binary Approve/Deny card and for Cancel — those carry the outcome in
/// `status` (`approved` / `denied` / `decided`).
pub struct ApprovalStatus {
    pub status: String,
    pub note: Option<String>,
    pub value: Option<String>,
    pub delivered_at: Option<f64>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create mailbox parent dir")?;
        }
        // Retry through SQLITE_BUSY during concurrent first-open. On a fresh
        // database, two processes calling `journal_mode=WAL` simultaneously
        // can each get SQLITE_BUSY before `busy_timeout` has taken effect.
        let mut last_err = None;
        let conn = {
            let mut got = None;
            for attempt in 0..20 {
                match try_open(path) {
                    Ok(c) => {
                        got = Some(c);
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        std::thread::sleep(std::time::Duration::from_millis(25 * (attempt + 1)));
                    }
                }
            }
            got.ok_or_else(|| {
                last_err.unwrap_or_else(|| anyhow::anyhow!("open sqlite: unknown error"))
            })?
        };
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

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
        let conn = self.conn.lock().unwrap();
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
        // non-text kinds, so the allowlist lives here.
        if kind == "system" && !sender.starts_with("system:") {
            bail!("kind 'system' may only be sent by a system:* source (got sender '{sender}')");
        }
        let conn = self.conn.lock().unwrap();
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

    /// Peek undelivered/unacked messages addressed directly to `agent_id` or
    /// to a channel `agent_id` subscribes to.
    pub fn inbox_peek(&self, agent_id: &str, limit: usize) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.project_id, m.sender, m.recipient, m.text, m.thread_id, m.sent_at,
                    m.telegram_msg_id, m.kind, m.structured_payload, m.delivery_mode
             FROM messages m
             WHERE m.acked_at IS NULL
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
            .query_map(params![agent_id, limit as i64], |r| {
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
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return the id of a currently-live bridge linking `from` and `to` (in
    /// either direction), or `None` if no live bridge authorizes the DM.
    pub fn live_bridge(&self, from: &str, to: &str) -> Result<Option<i64>> {
        let now = Self::now();
        let conn = self.conn.lock().unwrap();
        let row: Option<i64> = conn
            .query_row(
                "SELECT id FROM bridges
                 WHERE closed_at IS NULL
                   AND expires_at > ?1
                   AND ((from_agent = ?2 AND to_agent = ?3)
                     OR (from_agent = ?3 AND to_agent = ?2))
                 LIMIT 1",
                params![now, from, to],
                |r| r.get(0),
            )
            .ok();
        Ok(row)
    }

    /// Does `agent_id` have permission to DM `recipient_agent_id`?
    /// An empty `can_dm` list means unrestricted (any same-project agent).
    pub fn can_dm(&self, agent_id: &str, recipient_agent_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT can_dm_json FROM agent_acls WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .ok();
        let Some(json) = row else {
            return Ok(true); // no ACL row = unrestricted
        };
        let allowed: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        if allowed.is_empty() {
            return Ok(true);
        }
        let short = recipient_agent_id
            .split_once(':')
            .map(|(_, a)| a)
            .unwrap_or(recipient_agent_id);
        Ok(allowed
            .iter()
            .any(|a| a == short || a == recipient_agent_id))
    }

    /// Does `agent_id` have permission to post to channel `channel_name` in its project?
    pub fn can_broadcast(&self, agent_id: &str, channel_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT can_bcast_json FROM agent_acls WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .ok();
        let Some(json) = row else {
            return Ok(true);
        };
        let allowed: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        if allowed.is_empty() {
            return Ok(true);
        }
        Ok(allowed.iter().any(|c| c == channel_name))
    }

    /// Is `agent_id` a member of `channel_name` in its project?
    pub fn is_channel_member(
        &self,
        project: &str,
        channel_name: &str,
        agent_id: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let cid = format!("{project}:{channel_name}");
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND agent_id = ?2",
                params![cid, agent_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(n > 0)
    }

    /// Insert a broadcast message addressed to `channel:<project>:<name>`.
    pub fn send_broadcast(
        &self,
        project: &str,
        sender: &str,
        channel_name: &str,
        text: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let recipient = format!("channel:{project}:{channel_name}");
        conn.execute(
            "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project, sender, recipient, text, Self::now()],
        )?;
        Ok(conn.last_insert_rowid())
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        // Pull rows the caller is allowed to read AND that are still unacked.
        let select_sql = format!(
            "SELECT m.id, m.project_id, m.sender, m.recipient, m.text, m.thread_id, m.sent_at,
                    m.telegram_msg_id, m.kind, m.structured_payload, m.delivery_mode
             FROM messages m
             WHERE m.id IN ({placeholders})
               AND m.acked_at IS NULL
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
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 3);
        for id in ids {
            params_vec.push(Box::new(*id));
        }
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
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            collected
        };
        if !rows.is_empty() {
            let ack_ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
            let ack_placeholders = ack_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let ack_sql =
                format!("UPDATE messages SET acked_at = ? WHERE id IN ({ack_placeholders})");
            let mut ack_params: Vec<Box<dyn rusqlite::ToSql>> =
                Vec::with_capacity(ack_ids.len() + 1);
            ack_params.push(Box::new(Self::now()));
            for id in &ack_ids {
                ack_params.push(Box::new(*id));
            }
            let ack_refs: Vec<&dyn rusqlite::ToSql> = ack_params.iter().map(|b| &**b).collect();
            tx.execute(&ack_sql, ack_refs.as_slice())?;
        }
        tx.commit()?;
        // Preserve caller-supplied id order for predictability.
        let mut by_id: std::collections::HashMap<i64, Message> =
            rows.into_iter().map(|m| (m.id, m)).collect();
        let ordered = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        Ok(ordered)
    }

    /// Mark messages as acked.
    pub fn inbox_ack(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE messages SET acked_at = ?1 WHERE id IN ({placeholders}) AND acked_at IS NULL",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
        params_vec.push(Box::new(Self::now()));
        for id in ids {
            params_vec.push(Box::new(*id));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| &**b).collect();
        let n = conn.execute(&sql, refs.as_slice())?;
        Ok(n)
    }

    /// Return every agent in the caller's project. Used by `list_team`.
    pub fn list_project_agents(&self, project: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM agents WHERE project_id = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map(params![project], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return the project's org chart: managers (top tier) and per-worker
    /// `reports_to` links.
    pub fn org_chart(&self, project: &str) -> Result<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, runtime, is_manager, reports_to FROM agents WHERE project_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)? == 1,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let out = serde_json::json!({
            "project": project,
            "managers": rows.iter().filter(|r| r.3).map(|r| serde_json::json!({
                "id": r.0, "role": r.1, "runtime": r.2
            })).collect::<Vec<_>>(),
            "workers": rows.iter().filter(|r| !r.3).map(|r| serde_json::json!({
                "id": r.0, "role": r.1, "runtime": r.2, "reports_to": r.4
            })).collect::<Vec<_>>(),
        });
        Ok(out)
    }

    /// Insert a new pending approval request. Returns the id.
    #[allow(clippy::too_many_arguments)]
    pub fn request_approval(
        &self,
        project: &str,
        agent: &str,
        action: &str,
        scope_tag: Option<&str>,
        summary: &str,
        payload_json: &str,
        ttl_seconds: f64,
        options_json: Option<&str>,
    ) -> Result<i64> {
        let now = Self::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO approvals (project_id, agent_id, action, scope_tag, summary, payload_json, status, requested_at, expires_at, options_json)
             VALUES (?1,?2,?3,?4,?5,?6,'pending',?7,?8,?9)",
            params![project, agent, action, scope_tag, summary, payload_json, now, now + ttl_seconds, options_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Read status + optional note + chosen value + delivered_at for one
    /// approval request. `value` is `Some` only for a chosen multi-option
    /// decision (#299); binary Approve/Deny and Cancel leave it NULL and
    /// carry the outcome in `status`.
    pub fn approval_status(&self, id: i64) -> Result<ApprovalStatus> {
        let conn = self.conn.lock().unwrap();
        let s = conn.query_row(
            "SELECT status, decision_note, decision_value, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| {
                Ok(ApprovalStatus {
                    status: r.get(0)?,
                    note: r.get(1)?,
                    value: r.get(2)?,
                    delivered_at: r.get(3)?,
                })
            },
        )?;
        Ok(s)
    }

    /// Auto-expire pending approvals whose `expires_at` has passed. Rows that
    /// were never marked delivered transition to `undeliverable` so callers
    /// can distinguish "human didn't respond" from "the prompt never reached
    /// any human surface" — see decisions.md (T-031).
    pub fn expire_stale_approvals(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        let undeliverable = conn.execute(
            "UPDATE approvals SET status='undeliverable', decided_at=?1
             WHERE status='pending' AND expires_at < ?1 AND delivered_at IS NULL",
            params![now],
        )?;
        let expired = conn.execute(
            "UPDATE approvals SET status='expired', decided_at=?1
             WHERE status='pending' AND expires_at < ?1 AND delivered_at IS NOT NULL",
            params![now],
        )?;
        Ok(undeliverable + expired)
    }

    /// Mark an approval as delivered (i.e. surfaced to a human via some
    /// interface adapter). No-op if `delivered_at` is already set. Returns
    /// `true` when this call performed the flip. Wired up by interface
    /// adapters (`team-bot` et al.) in T-029/T-027 follow-up work.
    #[allow(dead_code)]
    pub fn mark_delivered(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE approvals SET delivered_at=?1
             WHERE id=?2 AND delivered_at IS NULL",
            params![Self::now(), id],
        )?;
        Ok(n > 0)
    }

    /// Decide one pending approval. `status` is the terminal bucket the
    /// caller resolved to — `"approved"`/`"denied"` for the binary card,
    /// `"decided"` for a chosen multi-option (#299), `"denied"` for
    /// Cancel. `value` is the chosen option's `value` (multi-option only;
    /// NULL for binary and Cancel — those carry the outcome in `status`).
    /// Returns `true` iff this call performed the decision (`false` =
    /// stale: the row was already terminal).
    ///
    /// Mirrors the bot's live in-process decision write (the bot owns its
    /// own rusqlite handle so it can't call this directly); kept coherent
    /// so the store-side decision contract is unit-testable. `teamctl
    /// approve/deny` writes directly with a canned SQL UPDATE.
    #[allow(dead_code)]
    pub fn decide_approval(
        &self,
        id: i64,
        status: &str,
        decided_by: &str,
        value: Option<&str>,
        note: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE approvals SET status=?1, decided_at=?2, decided_by=?3, decision_value=?4, decision_note=?5
             WHERE id=?6 AND status='pending'",
            params![status, Self::now(), decided_by, value, note, id],
        )?;
        Ok(n > 0)
    }

    /// Upsert project+agent registration rows. Idempotent.
    /// Consumed by `teamctl up` (Chunk C); keep pub even though `team-mcp`
    /// itself doesn't call it.
    #[allow(dead_code)]
    pub fn upsert_agent(
        &self,
        agent_id: &str,
        project_id: &str,
        project_name: &str,
        role: &str,
        runtime: &str,
        is_manager: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name) VALUES (?1, ?2)",
            params![project_id, project_name],
        )?;
        conn.execute(
            "INSERT INTO agents (id, project_id, role, runtime, is_manager)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               role = excluded.role,
               runtime = excluded.runtime,
               is_manager = excluded.is_manager",
            params![
                agent_id,
                project_id,
                role,
                runtime,
                if is_manager { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    /// Is `agent_id` registered as a manager (`is_manager = 1`)? Used to
    /// gate `reply_to_user` so only managers can talk back to the human.
    pub fn is_manager(&self, agent_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT is_manager FROM agents WHERE id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(n == 1)
    }

    /// Runtime adapter registered for `agent_id` (e.g. `claude-code`,
    /// `codex`). Returns `None` if the agent isn't registered. Used by
    /// `compact_self` to gate the `/compact` slash command on Claude
    /// Code, since other runtimes don't recognize that command shape.
    pub fn runtime_for(&self, agent_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let runtime = conn
            .query_row(
                "SELECT runtime FROM agents WHERE id = ?1",
                params![agent_id],
                |r| r.get::<_, String>(0),
            )
            .ok();
        Ok(runtime)
    }

    /// Unacked count for an agent. Used by `teamctl status`.
    #[allow(dead_code)]
    pub fn inbox_depth(&self, agent_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND acked_at IS NULL",
            params![agent_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn roundtrip_dm_and_ack() {
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .send_dm("hello", "hello:mgr", "hello:dev", "hi", None, None)
            .unwrap();
        let msgs = s.inbox_peek("hello:dev", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, id);
        assert_eq!(s.inbox_ack(&[id]).unwrap(), 1);
        assert!(s.inbox_peek("hello:dev", 10).unwrap().is_empty());
    }

    #[test]
    fn send_dm_kind_persists_kind_and_payload() {
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let payload = r#"{"source":"path","value":"/tmp/x.png","caption":"hi"}"#;
        let id = s
            .send_dm_kind(
                "p",
                "p:mgr",
                "user:telegram",
                "hi",
                None,
                "image",
                payload,
                None,
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let (kind, structured): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, structured_payload FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind.as_deref(), Some("image"));
        assert_eq!(structured.as_deref(), Some(payload));
    }

    #[test]
    fn send_dm_kind_system_from_system_sender_ok() {
        // #254: a `system:*` source may originate kind='system'.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .send_dm_kind(
                "p",
                "system:supervisor",
                "p:dev",
                "session terminating in 60s, call drained()",
                None,
                "system",
                "{}",
                None,
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let kind: Option<String> = conn
            .query_row(
                "SELECT kind FROM messages WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind.as_deref(), Some("system"));
    }

    #[test]
    fn send_dm_kind_system_from_agent_rejected() {
        // #254: an ordinary agent cannot forge a system signal.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let err = s
            .send_dm_kind(
                "p",
                "p:dev",
                "p:mgr",
                "fake drain",
                None,
                "system",
                "{}",
                None,
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("system:*"),
            "error must name the system:* restriction, got: {err}"
        );
        // The rejected message must not have been inserted.
        let conn = s.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected system insert must not persist a row");
    }

    #[test]
    fn send_dm_kind_system_from_user_rejected() {
        // #254: `user:*` cannot originate a system signal either.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let err = s
            .send_dm_kind(
                "p",
                "user:telegram",
                "p:dev",
                "fake startup",
                None,
                "system",
                "{}",
                None,
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("system:*"), "got: {err}");
    }

    #[test]
    fn send_dm_kind_non_system_kind_unaffected_by_allowlist() {
        // #254: the allowlist fires only for kind='system'. Existing
        // structured kinds from ordinary senders are untouched.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let payload = r#"{"source":"path","value":"/tmp/x.png"}"#;
        s.send_dm_kind(
            "p",
            "p:mgr",
            "user:telegram",
            "pic",
            None,
            "image",
            payload,
            None,
        )
        .expect("non-system kinds must not be gated by the system allowlist");
    }

    #[test]
    fn inbox_peek_surfaces_system_kind() {
        // #254 req 4: inbox tools pass system rows through unchanged.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        s.send_dm_kind(
            "p",
            "system:supervisor",
            "p:dev",
            "you were down for 12 minutes",
            None,
            "system",
            "{}",
            None,
        )
        .unwrap();
        let msgs = s.inbox_peek("p:dev", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind.as_deref(), Some("system"));
        assert_eq!(msgs[0].text, "you were down for 12 minutes");
    }

    #[test]
    fn request_approval_binary_leaves_options_json_null() {
        // Back-compat: a binary request (no options) must leave
        // options_json NULL so the bot takes the Approve/Deny path.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .request_approval("p", "p:dev", "deploy", None, "ship?", "{}", 900.0, None)
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let opts: Option<String> = conn
            .query_row(
                "SELECT options_json FROM approvals WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(opts.is_none(), "binary request must not set options_json");
    }

    #[test]
    fn request_approval_persists_options_json() {
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let opts = r#"[{"label":"A","value":"a"},{"label":"B","value":"b"}]"#;
        let id = s
            .request_approval(
                "p",
                "p:dev",
                "decide",
                None,
                "pick one",
                "{}",
                900.0,
                Some(opts),
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let stored: Option<String> = conn
            .query_row(
                "SELECT options_json FROM approvals WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some(opts));
    }

    #[test]
    fn decide_approval_records_chosen_value_and_is_idempotent() {
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let opts = r#"[{"label":"B","value":"opt_b"}]"#;
        let id = s
            .request_approval("p", "p:dev", "decide", None, "?", "{}", 900.0, Some(opts))
            .unwrap();

        // First decision wins and is recorded with the chosen value.
        let decided = s
            .decide_approval(id, "decided", "user:telegram", Some("opt_b"), None)
            .unwrap();
        assert!(decided, "first decision must report it performed the write");
        let st = s.approval_status(id).unwrap();
        assert_eq!(st.status, "decided");
        assert_eq!(st.value.as_deref(), Some("opt_b"));

        // A second tap against the now-terminal row is a no-op (stale).
        let again = s
            .decide_approval(id, "decided", "user:telegram", Some("other"), None)
            .unwrap();
        assert!(!again, "stale re-decide must report false");
        let value2 = s.approval_status(id).unwrap().value;
        assert_eq!(
            value2.as_deref(),
            Some("opt_b"),
            "value must not be overwritten"
        );
    }

    #[test]
    fn decide_approval_binary_path_leaves_value_null() {
        // The binary Approve/Deny outcome carries the result in `status`;
        // decision_value stays NULL (Cancel takes the same shape).
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .request_approval("p", "p:dev", "deploy", None, "ship?", "{}", 900.0, None)
            .unwrap();
        assert!(s
            .decide_approval(id, "approved", "user:telegram", None, None)
            .unwrap());
        let st = s.approval_status(id).unwrap();
        assert_eq!(st.status, "approved");
        assert!(st.value.is_none());
    }

    #[test]
    fn inbox_peek_surfaces_kind_and_payload_for_structured_rows() {
        // T-086-C contract: inbox_peek must surface the new columns so
        // an agent inspecting a structured row sees the path/caption it
        // needs to read bytes from disk. Without this, inbound media
        // would land in the database but be invisible to the agent.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let payload = r#"{"path":"/tmp/in/7.jpg","mime":"image/jpeg","size_bytes":1024}"#;
        s.send_dm_kind(
            "p",
            "user:telegram",
            "p:mgr",
            "look at this",
            None,
            "image",
            payload,
            None,
        )
        .unwrap();
        let msgs = s.inbox_peek("p:mgr", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind.as_deref(), Some("image"));
        assert_eq!(msgs[0].structured_payload.as_deref(), Some(payload));
        assert_eq!(msgs[0].text, "look at this", "caption mirrors into text");
    }

    #[test]
    fn inbox_peek_returns_null_kind_for_legacy_text_rows() {
        // Back-compat round-trip: a row inserted via the legacy
        // `send_dm` path comes back with `kind = None` and no payload,
        // which the agent's JSON parser will treat as a plain text
        // message exactly as before T-086-A.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        s.send_dm("p", "user:telegram", "p:mgr", "plain text", None, None)
            .unwrap();
        let msgs = s.inbox_peek("p:mgr", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].kind.is_none());
        assert!(msgs[0].structured_payload.is_none());
    }

    #[test]
    fn legacy_send_dm_leaves_kind_and_payload_null() {
        // Back-compat pin: existing text-only callers route through `send_dm`
        // unchanged. Both new columns must be NULL so readers that treat
        // NULL as 'text' (the bot dispatch path) don't accidentally route a
        // text row as media.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .send_dm("p", "p:mgr", "user:telegram", "hello", None, None)
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let (kind, structured): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, structured_payload FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(kind.is_none(), "legacy send_dm must leave kind NULL");
        assert!(
            structured.is_none(),
            "legacy send_dm must leave structured_payload NULL"
        );
    }

    /// Hand-insert an inbound row with a known `telegram_msg_id` and
    /// return its mailbox id. Mirrors what team-bot's inbound capture
    /// writes when an operator's Telegram message arrives.
    fn seed_inbound_row(s: &Store, telegram_msg_id: Option<i64>) -> i64 {
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, telegram_msg_id)
             VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, ?1)",
            params![telegram_msg_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn send_dm_resolves_mailbox_id_to_telegram_msg_id() {
        // T-168 happy path: agent passes the inbound row's mailbox id
        // (the value visible in the channel envelope as `meta.id`).
        // store.rs resolves it to the row's `telegram_msg_id` at insert
        // time and persists THAT on the outgoing row — what the bot's
        // outbound dispatcher actually feeds `reply_parameters` with.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let inbound_mailbox_id = seed_inbound_row(&s, Some(12345));
        let outbound_id = s
            .send_dm(
                "p",
                "p:mgr",
                "user:telegram",
                "ack",
                None,
                Some(inbound_mailbox_id),
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let stored: Option<i64> = conn
            .query_row(
                "SELECT telegram_msg_id FROM messages WHERE id = ?1",
                params![outbound_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, Some(12345));
    }

    #[test]
    fn send_dm_leaves_telegram_msg_id_null_when_omitted() {
        // Back-compat pin: callers that don't pass a reply target leave
        // the column NULL so the bot's dispatcher omits
        // `reply_parameters` and the message lands as a fresh post.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .send_dm("p", "p:mgr", "user:telegram", "ack", None, None)
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let stored: Option<i64> = conn
            .query_row(
                "SELECT telegram_msg_id FROM messages WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.is_none());
    }

    #[test]
    fn send_dm_resolution_misses_when_row_has_no_telegram_msg_id() {
        // T-168 miss case A: agent references an inbound row that
        // exists but isn't a Telegram-sourced row (telegram_msg_id is
        // NULL on peer-to-peer or system rows). Resolution returns
        // None; the outgoing row sends without threading rather than
        // failing the whole call.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let inbound_mailbox_id = seed_inbound_row(&s, None);
        let outbound_id = s
            .send_dm(
                "p",
                "p:mgr",
                "user:telegram",
                "ack",
                None,
                Some(inbound_mailbox_id),
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let stored: Option<i64> = conn
            .query_row(
                "SELECT telegram_msg_id FROM messages WHERE id = ?1",
                params![outbound_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.is_none());
    }

    #[test]
    fn send_dm_resolution_misses_when_mailbox_id_does_not_exist() {
        // T-168 miss case B: agent passes a mailbox id that points at
        // no row at all (stale id, off-by-one, etc.). Same posture:
        // resolution misses, the send still goes through unthreaded.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let outbound_id = s
            .send_dm("p", "p:mgr", "user:telegram", "ack", None, Some(999_999))
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let stored: Option<i64> = conn
            .query_row(
                "SELECT telegram_msg_id FROM messages WHERE id = ?1",
                params![outbound_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.is_none());
    }

    #[test]
    fn send_dm_kind_resolves_mailbox_id_to_telegram_msg_id() {
        // T-168 happy path for media: image attached as a reply
        // resolves the mailbox id the same way `send_dm` does. Kind +
        // payload + resolved telegram_msg_id all persist on one row.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let inbound_mailbox_id = seed_inbound_row(&s, Some(99));
        let outbound_id = s
            .send_dm_kind(
                "p",
                "p:mgr",
                "user:telegram",
                "screenshot",
                None,
                "image",
                r#"{"source":"url","value":"https://x.test/a.png"}"#,
                Some(inbound_mailbox_id),
            )
            .unwrap();
        let conn = s.conn.lock().unwrap();
        let (kind, telegram_msg_id): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT kind, telegram_msg_id FROM messages WHERE id = ?1",
                params![outbound_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind.as_deref(), Some("image"));
        assert_eq!(telegram_msg_id, Some(99));
    }

    #[test]
    fn inbox_peek_surfaces_telegram_msg_id_field() {
        // Agent-side: the field on the `Message` struct is what the MCP
        // tool surfaces back to the caller, so the agent can read which
        // Telegram id the operator's message had and thread its reply.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        // Hand-insert an inbound row with telegram_msg_id set, mimicking
        // what team-bot's inbound capture would write.
        {
            let conn = s.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO projects (id, name) VALUES ('p','P')
                 ON CONFLICT(id) DO NOTHING",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agents (id, project_id, role, runtime, is_manager, reports_to)
                 VALUES ('p:mgr','p','manager','claude-code',1,NULL)
                 ON CONFLICT(id) DO NOTHING",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages
                    (project_id, sender, recipient, text, sent_at, telegram_msg_id)
                 VALUES ('p', 'user:telegram', 'p:mgr', 'hello', 0.0, 4242)",
                [],
            )
            .unwrap();
        }
        let msgs = s.inbox_peek("p:mgr", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].telegram_msg_id,
            Some(4242),
            "inbox_peek must expose the captured Telegram id to the agent"
        );
    }

    #[test]
    fn upsert_agent_is_idempotent() {
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        s.upsert_agent(
            "hello:mgr",
            "hello",
            "Hello",
            "product-mgr",
            "claude-code",
            true,
        )
        .unwrap();
        s.upsert_agent(
            "hello:mgr",
            "hello",
            "Hello",
            "product-mgr",
            "claude-code",
            true,
        )
        .unwrap();
        s.upsert_agent("hello:dev", "hello", "Hello", "dev", "claude-code", false)
            .unwrap();
        assert_eq!(
            s.list_project_agents("hello").unwrap(),
            vec!["hello:dev".to_string(), "hello:mgr".into()]
        );
    }
}
