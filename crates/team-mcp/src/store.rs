//! SQLite-backed message store. One connection per process.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

mod access;
mod agents;
mod approvals;
mod inbox;
mod messaging;

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

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("mailbox connection lock poisoned"))
    }
}

#[cfg(test)]
mod tests;
