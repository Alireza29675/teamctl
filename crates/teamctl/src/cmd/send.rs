//! `teamctl send <project>:<agent> "text"` — inject a message as `sender=cli`.

use std::path::Path;

use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(root: &Path, target: &str, text: &str) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let db = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(team_core::mailbox::SCHEMA)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at) VALUES (?1,?2,?3,?4,?5)",
        params![handle.project, "cli", target, text, now],
    )?;
    println!("sent · {target}");
    Ok(())
}
