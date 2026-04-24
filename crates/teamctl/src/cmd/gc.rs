//! `teamctl gc` — clean up expired messages and stale approvals.

use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        println!("(nothing to gc — no mailbox)");
        return Ok(());
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(team_core::mailbox::SCHEMA)?;

    let ttl_hours = compose.global.budget.message_ttl_hours.unwrap_or(24) as f64;
    let horizon = now() - ttl_hours * 3600.0;
    let msgs = conn.execute(
        "DELETE FROM messages WHERE sent_at < ?1 AND acked_at IS NOT NULL",
        params![horizon],
    )?;
    let approvals = conn.execute(
        "UPDATE approvals SET status='expired', decided_at=?1
         WHERE status='pending' AND expires_at < ?1",
        params![now()],
    )?;
    println!("gc · {msgs} acked messages removed · {approvals} approvals expired");
    Ok(())
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
