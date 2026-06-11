//! `teamctl rl-hit <project>:<agent>`
//!
//! Records a single rate-limit hit row from outside the `rl-watch` pty.
//! The `StopFailure` + `matcher:"rate_limit"` Claude Code hook (rendered by
//! `team-core::render`) invokes this when a turn ends on a rate-limit stop -
//! a moment `rl-watch` can't see, because an interactive headless session is
//! driven by Claude Code itself, not wrapped in our pty.
//!
//! The row is a forensic marker only: it carries no `resets_at` (the hook has
//! no PTY output to parse a reset time from), so it's stored as NULL. The TUI
//! countdown reads `MAX(id) ... WHERE resets_at IS NOT NULL`, so a NULL row is
//! invisible to the indicator - it never displaces an `rl-watch` countdown row
//! and never fabricates a window of its own. It just lets the hit be counted.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection};

/// Forensic marker stored in `raw_match` so a hook-sourced hit is
/// distinguishable from an `rl-watch` pattern match in the table.
const RAW_MARKER: &str = "stopfailure:rate_limit";

/// Fallback runtime when the agent isn't found in the compose (e.g. it was
/// renamed since the settings file was rendered). The hook is claude-code
/// only, so a constant is a safe forensic stand-in.
const DEFAULT_RUNTIME: &str = "claude-code";

pub fn run(root: &Path, agent: &str) -> Result<()> {
    let compose = super::load(root)?;
    let db_path = compose.root.join(&compose.global.broker.path);

    // Resolve the runtime from the compose if the agent is still declared;
    // otherwise fall back to the claude-code constant (this hook is
    // claude-code only). Cheap lookup, no runtime-def load needed.
    let runtime = compose
        .agents()
        .find(|h| h.id() == agent)
        .map(|h| h.spec.runtime.clone())
        .unwrap_or_else(|| DEFAULT_RUNTIME.to_string());

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&db_path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;

    record_hit(&conn, agent, &runtime, now())?;

    // Quiet by default: a forensic marker hook runs on every rate-limit stop,
    // and its stderr can be surfaced into the agent's context for some hook
    // events. The recorded row is the durable signal; a success line would be
    // per-stop noise. `tracing::debug!` keeps it inspectable without leaking.
    tracing::debug!(agent, "recorded rate-limit hit");
    Ok(())
}

/// Insert one forensic rate-limit hit row.
///
/// Always stores `resets_at = NULL` (the hook has no PTY output to parse a
/// reset time from) and `raw_match = RAW_MARKER`. The NULL `resets_at` is the
/// load-bearing invariant: it keeps the row invisible to the TUI countdown,
/// which reads `MAX(id) ... WHERE resets_at IS NOT NULL`.
fn record_hit(conn: &Connection, agent: &str, runtime: &str, hit_at: f64) -> Result<()> {
    conn.execute(
        "INSERT INTO rate_limits (agent_id, runtime, hit_at, resets_at, raw_match)
         VALUES (?1,?2,?3,?4,?5)",
        params![agent, runtime, hit_at, None::<f64>, RAW_MARKER],
    )?;
    Ok(())
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a schema-bootstrapped in-memory db so the test exercises the
    /// real `rate_limits` table shape, not a hand-rolled subset.
    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        team_core::mailbox::ensure(&conn).expect("bootstrap schema");
        conn
    }

    #[test]
    fn record_hit_writes_a_forensic_marker_row_with_null_resets_at() {
        let conn = db();
        record_hit(&conn, "alpha:dev", "claude-code", 123.0).expect("record hit");

        let (agent_id, runtime, hit_at, resets_at, raw_match): (
            String,
            String,
            f64,
            Option<f64>,
            String,
        ) = conn
            .query_row(
                "SELECT agent_id, runtime, hit_at, resets_at, raw_match FROM rate_limits",
                [],
                |row| {
                    Ok((
                        row.get("agent_id")?,
                        row.get("runtime")?,
                        row.get("hit_at")?,
                        row.get("resets_at")?,
                        row.get("raw_match")?,
                    ))
                },
            )
            .expect("query the inserted row");

        assert_eq!(agent_id, "alpha:dev");
        assert_eq!(runtime, "claude-code");
        assert_eq!(hit_at, 123.0);
        assert_eq!(raw_match, "stopfailure:rate_limit");
        assert_eq!(raw_match, RAW_MARKER);
        // Load-bearing invariant: `resets_at` is stored NULL so the forensic
        // row stays invisible to the TUI countdown, which filters
        // `WHERE resets_at IS NOT NULL` (see teamctl-ui `data.rs`). A non-null
        // value here would fabricate a phantom countdown window.
        assert_eq!(
            resets_at, None,
            "resets_at must be NULL for a hook-sourced hit"
        );
    }

    #[test]
    fn record_hit_does_not_dedup_repeated_hits() {
        // The table carries no unique constraint by design: every stop on a
        // rate-limit is its own countable forensic event, so two calls for the
        // same agent produce two distinct rows rather than colliding.
        let conn = db();
        record_hit(&conn, "alpha:dev", "claude-code", 100.0).expect("first hit");
        record_hit(&conn, "alpha:dev", "claude-code", 200.0).expect("second hit");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rate_limits", [], |row| row.get(0))
            .expect("count rows");
        assert_eq!(count, 2, "two hits must produce two rows, no dedup");
    }
}
