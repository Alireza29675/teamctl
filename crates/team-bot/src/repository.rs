use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::transport::MailboxRow;

pub(super) fn open_mailbox(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create mailbox parent")?;
    }
    let conn = Connection::open(path).context("open mailbox")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    team_core::mailbox::ensure(&conn)?;
    Ok(conn)
}

pub(super) fn decide_approval(
    conn: &Connection,
    id: i64,
    status: &str,
    value: Option<&str>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'),
             decided_by='user:telegram', decision_value=?2,
             delivered_at=COALESCE(delivered_at, strftime('%s','now'))
         WHERE id=?3 AND status='pending' AND expires_at > strftime('%s','now')",
        params![status, value, id],
    )? > 0)
}

pub(super) fn mark_approval_delivered(conn: &Connection, id: i64) -> Result<bool> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs_f64();
    Ok(conn.execute(
        "UPDATE approvals SET delivered_at=?2
         WHERE id=?1 AND delivered_at IS NULL AND status='pending'
           AND expires_at > ?2",
        params![id, now],
    )? > 0)
}

pub(super) type ApprovalRow = (i64, String, String, String, Option<String>);

pub(super) fn pending_approvals(
    conn: &Connection,
    project: Option<&str>,
) -> Result<Vec<ApprovalRow>> {
    active_approvals(conn, project, true)
}

pub(super) fn active_approvals(
    conn: &Connection,
    project: Option<&str>,
    undelivered_only: bool,
) -> Result<Vec<ApprovalRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, action, summary, options_json FROM approvals
         WHERE status='pending' AND (?2 = 0 OR delivered_at IS NULL)
           AND expires_at > strftime('%s','now')
           AND (?1 IS NULL OR project_id = ?1) ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![project, undelivered_only], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(super) fn outgoing_messages(
    conn: &Connection,
    project: Option<&str>,
) -> Result<Vec<MailboxRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, sender, text, kind, structured_payload, telegram_msg_id FROM messages
         WHERE recipient='user:telegram' AND acked_at IS NULL
           AND conversation_id IS NULL
           AND (kind IS NULL OR kind != 'media_pending')
           AND (?1 IS NULL OR project_id = ?1) ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![project], MailboxRow::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(super) fn manager_of(conn: &Connection, agent_id: &str) -> Option<String> {
    let row: Option<(String, i64, Option<String>)> = conn
        .query_row(
            "SELECT project_id, is_manager, reports_to FROM agents WHERE id = ?1",
            params![agent_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let (project, is_manager, reports_to) = row?;
    if is_manager == 1 {
        return Some(agent_id.to_string());
    }
    let role = reports_to?;
    Some(format!("{project}:{role}"))
}

/// Route an approval row to *this* bot iff:
/// - `scoped` is `None` (unscoped bot — back-compat fallback for setups
///   that predate per-manager scoping; surface every approval), or
/// - `scoped` is `Some(<project>:<manager>)` and the agent that filed
///   the approval rolls up to that manager (per `manager_of`).
///
/// Pulled out as a free function so the unscoped-vs-scoped semantics
/// are unit-testable without spinning up an async tokio runtime.
pub(super) fn should_route(scoped: Option<&str>, agent_id: &str, conn: &Connection) -> bool {
    let Some(scoped) = scoped else {
        return true;
    };
    let routed = manager_of(conn, agent_id).unwrap_or_else(|| agent_id.to_string());
    routed == scoped
}

/// Look up the registered runtime for an agent. Used by slash-passthrough
/// (T-086-G) to feature-gate the chord on `runtime: claude-code` and by
/// the setMyCommands registration (T-086-H) to pick the per-runtime
/// command list. Returns `None` if the agent isn't in the mailbox's
/// `agents` table.
pub(super) fn agent_runtime(conn: &Connection, agent_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT runtime FROM agents WHERE id = ?1",
        params![agent_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}
