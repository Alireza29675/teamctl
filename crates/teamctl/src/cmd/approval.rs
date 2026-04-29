//! `teamctl pending / approve / deny` — decide pending HITL approvals.

use std::path::Path;

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

pub fn pending(root: &Path) -> Result<()> {
    let conn = open_db(root)?;
    let mut stmt = conn.prepare(
        "SELECT id, project_id, agent_id, action, summary
         FROM approvals WHERE status='pending' ORDER BY id ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut found = false;
    println!(
        "{:<5} {:<14} {:<22} {:<14} SUMMARY",
        "ID", "PROJECT", "AGENT", "ACTION"
    );
    while let Some(r) = rows.next()? {
        found = true;
        let id: i64 = r.get(0)?;
        let project: String = r.get(1)?;
        let agent: String = r.get(2)?;
        let action: String = r.get(3)?;
        let summary: String = r.get(4)?;
        println!("{id:<5} {project:<14} {agent:<22} {action:<14} {summary}");
    }
    if !found {
        println!("(no pending approvals)");
    }
    Ok(())
}

pub fn decide(root: &Path, id: i64, approved: bool, note: Option<&str>) -> Result<()> {
    let conn = open_db(root)?;
    let status = if approved { "approved" } else { "denied" };
    // A CLI decision is itself a delivery acknowledgement: the operator saw
    // the prompt on some surface (otherwise they couldn't decide). Flip
    // delivered_at when null so the row's lifecycle stays truthful.
    conn.execute(
        "UPDATE approvals SET delivered_at=strftime('%s','now')
         WHERE id=?1 AND delivered_at IS NULL",
        params![id],
    )?;
    let n = conn.execute(
        "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'), decided_by='cli', decision_note=?2
         WHERE id=?3 AND status='pending'",
        params![status, note, id],
    )?;
    if n == 0 {
        bail!("no pending approval with id {id}");
    }
    println!("approval {id} {status}");
    Ok(())
}
