//! SQLite-backed message store. One connection per process.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

pub struct Store {
    pub(crate) conn: Mutex<Connection>,
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

    /// Insert a DM (recipient is `<project>:<agent>`).
    pub fn send_dm(
        &self,
        project: &str,
        sender: &str,
        recipient: &str,
        text: &str,
        thread_id: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (project_id, sender, recipient, text, thread_id, sent_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project, sender, recipient, text, thread_id, Self::now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a structured-content DM (image/file/etc.). `kind` is the
    /// discriminator the bot's outbound dispatcher matches on; `payload` is
    /// the JSON-encoded content descriptor (e.g. `{"source":"path",
    /// "value":"/tmp/x.png","caption":"…"}`). The `text` column carries the
    /// caption (if any) so legacy text-only readers still see something
    /// meaningful; the structured payload is the source of truth for
    /// dispatch.
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
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, thread_id, sent_at, kind, structured_payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project,
                sender,
                recipient,
                text,
                thread_id,
                Self::now(),
                kind,
                payload,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Peek undelivered/unacked messages addressed directly to `agent_id` or
    /// to a channel `agent_id` subscribes to.
    pub fn inbox_peek(&self, agent_id: &str, limit: usize) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.project_id, m.sender, m.recipient, m.text, m.thread_id, m.sent_at
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
    ) -> Result<i64> {
        let now = Self::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO approvals (project_id, agent_id, action, scope_tag, summary, payload_json, status, requested_at, expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,'pending',?7,?8)",
            params![project, agent, action, scope_tag, summary, payload_json, now, now + ttl_seconds],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Read status + optional note + delivered_at for one approval request.
    pub fn approval_status(&self, id: i64) -> Result<(String, Option<String>, Option<f64>)> {
        let conn = self.conn.lock().unwrap();
        let (status, note, delivered_at): (String, Option<String>, Option<f64>) = conn.query_row(
            "SELECT status, decision_note, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok((status, note, delivered_at))
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

    /// Decide one approval. Used by interface adapters (`team-bot` et al.);
    /// `teamctl approve/deny` writes directly with a canned SQL UPDATE.
    #[allow(dead_code)]
    pub fn decide_approval(
        &self,
        id: i64,
        approved: bool,
        decided_by: &str,
        note: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let status = if approved { "approved" } else { "denied" };
        conn.execute(
            "UPDATE approvals SET status=?1, decided_at=?2, decided_by=?3, decision_note=?4
             WHERE id=?5 AND status='pending'",
            params![status, Self::now(), decided_by, note, id],
        )?;
        Ok(())
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
            .send_dm("hello", "hello:mgr", "hello:dev", "hi", None)
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
            .send_dm_kind("p", "p:mgr", "user:telegram", "hi", None, "image", payload)
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
    fn legacy_send_dm_leaves_kind_and_payload_null() {
        // Back-compat pin: existing text-only callers route through `send_dm`
        // unchanged. Both new columns must be NULL so readers that treat
        // NULL as 'text' (the bot dispatch path) don't accidentally route a
        // text row as media.
        let f = NamedTempFile::new().unwrap();
        let s = Store::open(f.path()).unwrap();
        let id = s
            .send_dm("p", "p:mgr", "user:telegram", "hello", None)
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
