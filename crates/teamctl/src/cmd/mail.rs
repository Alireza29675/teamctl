//! `teamctl mail [target] [--all]` — inbox snapshots.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

pub fn run(root: &Path, target: Option<&str>, all: bool) -> Result<()> {
    let compose = super::load(root)?;
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        println!("(no mailbox yet — run `teamctl up`)");
        return Ok(());
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(Duration::from_secs(5))?;

    if all {
        return print_all(&conn, &compose);
    }
    let Some(target) = target else {
        bail!("provide an agent id like `<project>:<agent>` or pass --all");
    };
    if !compose.agents().any(|h| h.id() == target) {
        bail!("no such agent: {target}");
    }
    print_inbox(&conn, target)
}

fn print_inbox(conn: &Connection, agent: &str) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.sender, m.text, m.sent_at, m.acked_at
         FROM messages m
         WHERE m.recipient = ?1
            OR m.recipient IN (
                SELECT 'channel:' || cm.channel_id FROM channel_members cm WHERE cm.agent_id = ?1
            )
         ORDER BY m.id DESC LIMIT 50",
    )?;
    let mut rows = stmt.query(params![agent])?;
    println!("{:<5} {:<6} {:<24} SUMMARY", "ID", "STATE", "FROM");
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let sender: String = r.get(1)?;
        let text: String = r.get(2)?;
        let acked: Option<f64> = r.get(4)?;
        let state = if acked.is_some() { "read" } else { "new" };
        let summary = first_line(&text, 80);
        println!("{id:<5} {state:<6} {sender:<24} {summary}");
    }
    Ok(())
}

fn print_all(conn: &Connection, compose: &team_core::compose::Compose) -> Result<()> {
    println!("{:<28} {:<6} LATEST", "AGENT", "UNACK");
    for h in compose.agents() {
        let id = h.id();
        let unack: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE acked_at IS NULL
               AND (recipient = ?1
                    OR recipient IN (SELECT 'channel:' || channel_id FROM channel_members WHERE agent_id = ?1))",
            params![id],
            |r| r.get(0),
        ).unwrap_or(0);
        let latest: Option<String> = conn
            .query_row(
                "SELECT text FROM messages
             WHERE recipient = ?1 OR recipient IN (
                 SELECT 'channel:' || channel_id FROM channel_members WHERE agent_id = ?1
             )
             ORDER BY id DESC LIMIT 1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        let preview = latest
            .as_deref()
            .map(|t| first_line(t, 60))
            .unwrap_or_else(|| "—".into());
        println!("{id:<28} {unack:<6} {preview}");
    }
    Ok(())
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > max {
        let mut out: String = line.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        line.to_string()
    }
}
