//! Operator broadcasts to a compose channel, with explicit project routing.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use team_core::compose::Compose;

fn resolve_channel(compose: &Compose, target: &str) -> Result<(String, String)> {
    let target = target.strip_prefix("channel:").unwrap_or(target);
    let (project, name) = match target.split_once(':') {
        Some((project, name)) => (Some(project), name.trim_start_matches('#')),
        None => (None, target.trim_start_matches('#')),
    };
    let mut matches = compose.projects.iter().filter_map(|p| {
        if project.is_some_and(|id| id != p.project.id) {
            return None;
        }
        p.channels
            .iter()
            .find(|c| c.name == name)
            .map(|c| (p.project.id.clone(), c.name.clone()))
    });
    let Some(channel) = matches.next() else {
        bail!("no such channel: {target}");
    };
    if matches.next().is_some() {
        bail!("ambiguous channel: {target}; use <project>:<channel>");
    }
    Ok(channel)
}

pub fn run(root: &Path, target: &str, text: &str) -> Result<()> {
    let compose = super::load(root)?;
    let (project, channel) = resolve_channel(&compose, target)?;
    let db = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;
    let recipient = format!("channel:{project}:{channel}");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
         VALUES (?1, 'cli', ?2, ?3, ?4)",
        params![project, recipient, text, now],
    )?;
    println!("broadcast · {recipient}");
    Ok(())
}
