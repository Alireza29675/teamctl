//! SQLite-backed message store. One connection per process.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

pub struct Store {
    conn: Mutex<Connection>,
}

fn try_open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).context("open sqlite")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(team_core::mailbox::SCHEMA)?;
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

    /// Peek undelivered/unacked messages addressed to `agent_id`.
    pub fn inbox_peek(&self, agent_id: &str, limit: usize) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, sender, recipient, text, thread_id, sent_at
             FROM messages
             WHERE recipient = ?1 AND acked_at IS NULL
             ORDER BY id ASC
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
