//! `teamctl tail <agent> [-f]` — stream messages addressed to or from an agent.

use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

pub fn run(root: &Path, target: &str, follow: bool) -> Result<()> {
    let compose = super::load(root)?;
    if !compose.agents().any(|h| h.id() == target) {
        bail!("no such agent: {target}");
    }
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        bail!("no mailbox at {} — run `teamctl up` first", db.display());
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(Duration::from_secs(5))?;

    let mut after: i64 = 0;

    loop {
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text, sent_at FROM messages
             WHERE id > ?1 AND (sender = ?2 OR recipient = ?2
                 OR recipient IN (
                     SELECT 'channel:' || cm.channel_id FROM channel_members cm
                     WHERE cm.agent_id = ?2
                 ))
             ORDER BY id ASC",
        )?;
        let mut rows = stmt.query(params![after, target])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let sender: String = r.get(1)?;
            let recipient: String = r.get(2)?;
            let text: String = r.get(3)?;
            let direction = if sender == target { "→" } else { "←" };
            let other = if sender == target { recipient } else { sender };
            println!("#{id}  {direction} {other}");
            for line in text.lines() {
                println!("    {line}");
            }
            after = id;
        }
        if !follow {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
}
