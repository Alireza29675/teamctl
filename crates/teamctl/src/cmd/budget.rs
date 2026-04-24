//! `teamctl budget` — aggregate activity and cost per project for today.
//!
//! Phase 7 ships the plumbing: a `budget` table, a sane aggregate query, and
//! the command surface. Runtime cost parsers (Claude `/cost`, Codex per-msg
//! totals, Gemini summary) feed rows in follow-up work — `budget` already
//! accepts the schema they need.

use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

pub fn run(root: &Path, project: Option<&str>) -> Result<()> {
    let compose = super::load(root)?;
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        println!("(no mailbox yet — run `teamctl up`)");
        return Ok(());
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(team_core::mailbox::SCHEMA)?;

    let today_start = midnight_utc();

    let projects: Vec<String> = match project {
        Some(p) => vec![p.to_string()],
        None => {
            let mut stmt = conn.prepare("SELECT id FROM projects ORDER BY id")?;
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        }
    };

    println!(
        "{:<18} {:>10} {:>14} {:>10} {:>10} LIMIT",
        "PROJECT", "MSGS-24H", "APPROVALS-24H", "USD-24H", "AGENTS"
    );
    for pid in &projects {
        let msg_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE project_id = ?1 AND sent_at >= ?2",
            params![pid, today_start],
            |r| r.get(0),
        )?;
        let appr_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM approvals WHERE project_id = ?1 AND requested_at >= ?2",
            params![pid, today_start],
            |r| r.get(0),
        )?;
        let usd: f64 = conn.query_row(
            "SELECT COALESCE(SUM(usd), 0) FROM budget WHERE project_id = ?1 AND observed_at >= ?2",
            params![pid, today_start],
            |r| r.get(0),
        )?;
        let agents: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE project_id = ?1",
            params![pid],
            |r| r.get(0),
        )?;
        let limit = compose
            .global
            .budget
            .per_project_usd_limit
            .get(pid)
            .map(|v| format!("${v:.2}"))
            .or_else(|| {
                compose
                    .global
                    .budget
                    .daily_usd_limit
                    .map(|v| format!("${v:.2} (global)"))
            })
            .unwrap_or_else(|| "—".into());
        println!("{pid:<18} {msg_count:>10} {appr_count:>14} {usd:>9.2}$ {agents:>10}  {limit}");
    }
    Ok(())
}

fn midnight_utc() -> f64 {
    // Seconds since Unix epoch at UTC midnight today.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    // 86400 s per day
    (now / 86400.0).floor() * 86400.0
}
