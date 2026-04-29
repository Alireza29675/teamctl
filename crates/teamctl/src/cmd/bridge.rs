//! `teamctl bridge open/close/list/log` — manage inter-project manager bridges.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

pub fn open_db(root: &Path) -> Result<Connection> {
    let compose = super::load(root)?;
    let db = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;
    Ok(conn)
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn open(root: &Path, from: &str, to: &str, topic: &str, ttl_min: u64) -> Result<()> {
    let conn = open_db(root)?;
    // Both endpoints must exist + be managers.
    let is_manager = |id: &str| -> Result<bool> {
        let row: Option<i64> = conn
            .query_row(
                "SELECT is_manager FROM agents WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        Ok(row == Some(1))
    };
    if !is_manager(from)? {
        bail!("{from} is not a registered manager");
    }
    if !is_manager(to)? {
        bail!("{to} is not a registered manager");
    }
    let from_proj = from.split_once(':').map(|(p, _)| p).unwrap_or("");
    let to_proj = to.split_once(':').map(|(p, _)| p).unwrap_or("");
    if from_proj == to_proj {
        bail!("bridges are for inter-project links; {from} and {to} are in the same project");
    }
    let opened = now();
    let expires = opened + (ttl_min as f64) * 60.0;
    conn.execute(
        "INSERT INTO bridges (from_agent, to_agent, topic, opened_by, opened_at, expires_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![from, to, topic, "cli", opened, expires],
    )?;
    let id = conn.last_insert_rowid();
    println!("bridge {id} open · {from} ↔ {to} · topic: {topic} · expires in {ttl_min}m");
    Ok(())
}

pub fn close(root: &Path, id: i64) -> Result<()> {
    let conn = open_db(root)?;
    let n = conn.execute(
        "UPDATE bridges SET closed_at = ?1 WHERE id = ?2 AND closed_at IS NULL",
        params![now(), id],
    )?;
    if n == 0 {
        bail!("no open bridge with id {id}");
    }
    println!("bridge {id} closed");
    Ok(())
}

pub fn list(root: &Path) -> Result<()> {
    let conn = open_db(root)?;
    let n = now();
    let mut stmt = conn.prepare(
        "SELECT id, from_agent, to_agent, topic, opened_at, expires_at, closed_at
         FROM bridges ORDER BY id DESC",
    )?;
    let mut rows = stmt.query([])?;
    println!(
        "{:<5} {:<30} {:<30} {:<10} TOPIC",
        "ID", "FROM", "TO", "STATE"
    );
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let from: String = r.get(1)?;
        let to: String = r.get(2)?;
        let topic: String = r.get(3)?;
        let expires: f64 = r.get(5)?;
        let closed: Option<f64> = r.get(6)?;
        let state = if closed.is_some() {
            "closed"
        } else if expires <= n {
            "expired"
        } else {
            "open"
        };
        println!("{id:<5} {from:<30} {to:<30} {state:<10} {topic}");
    }
    Ok(())
}

pub fn log(root: &Path, id: i64) -> Result<()> {
    let conn = open_db(root)?;
    let mut stmt = conn.prepare(
        "SELECT sender, recipient, text, sent_at FROM messages
         WHERE thread_id = ?1 ORDER BY id ASC",
    )?;
    let thread = format!("bridge:{id}");
    let mut rows = stmt.query(params![thread])?;
    while let Some(r) = rows.next()? {
        let sender: String = r.get(0)?;
        let recipient: String = r.get(1)?;
        let text: String = r.get(2)?;
        println!("  {sender}  →  {recipient}: {text}");
    }
    Ok(())
}
