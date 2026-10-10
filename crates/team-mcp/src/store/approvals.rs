use anyhow::Result;
use rusqlite::params;

use super::{ApprovalStatus, Store};

impl Store {
    /// Insert a new pending approval request. Returns the id.
    #[allow(clippy::too_many_arguments)]
    pub fn request_approval(
        &self,
        project: &str,
        agent: &str,
        action: &str,
        scope_tag: Option<&str>,
        summary: &str,
        payload_json: &str,
        ttl_seconds: f64,
        options_json: Option<&str>,
    ) -> Result<i64> {
        let now = Self::now();
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO approvals (project_id, agent_id, action, scope_tag, summary, payload_json, status, requested_at, expires_at, options_json)
             VALUES (?1,?2,?3,?4,?5,?6,'pending',?7,?8,?9)",
            params![project, agent, action, scope_tag, summary, payload_json, now, now + ttl_seconds, options_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Read status + optional note + chosen value + delivered_at for one
    /// approval request. `value` is `Some` only for a chosen multi-option
    /// decision (#299); binary Approve/Deny and Cancel leave it NULL and
    /// carry the outcome in `status`.
    pub fn approval_status(&self, id: i64) -> Result<ApprovalStatus> {
        let conn = self.connection()?;
        let s = conn.query_row(
            "SELECT status, decision_note, decision_value, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| {
                Ok(ApprovalStatus {
                    status: r.get(0)?,
                    note: r.get(1)?,
                    value: r.get(2)?,
                    delivered_at: r.get(3)?,
                })
            },
        )?;
        Ok(s)
    }

    /// Auto-expire pending approvals whose `expires_at` has passed. Rows that
    /// were never marked delivered transition to `undeliverable` so callers
    /// can distinguish "human didn't respond" from "the prompt never reached
    /// any human surface" — see decisions.md (T-031).
    pub fn expire_stale_approvals(&self) -> Result<usize> {
        let conn = self.connection()?;
        let now = Self::now();
        let undeliverable = conn.execute(
            "UPDATE approvals SET status='undeliverable', decided_at=?1
             WHERE status='pending' AND expires_at < ?1 AND delivered_at IS NULL",
            params![now],
        )?;
        let expired = conn.execute(
            "UPDATE approvals SET status='expired', decided_at=?1
             WHERE status='pending' AND expires_at < ?1 AND delivered_at IS NOT NULL",
            params![now],
        )?;
        Ok(undeliverable + expired)
    }

    /// Mark an approval as delivered (i.e. surfaced to a human via some
    /// interface adapter). Only a live pending approval can be marked; no-op
    /// if `delivered_at` is already set, expired, or resolved. Returns
    /// `true` when this call performed the flip. Wired up by interface
    /// adapters (`team-bot` et al.) in T-029/T-027 follow-up work.
    #[allow(dead_code)]
    pub fn mark_delivered(&self, id: i64) -> Result<bool> {
        let conn = self.connection()?;
        let n = conn.execute(
            "UPDATE approvals SET delivered_at=?1
             WHERE id=?2 AND delivered_at IS NULL AND status='pending'
               AND expires_at > ?1",
            params![Self::now(), id],
        )?;
        Ok(n > 0)
    }

    /// Decide one pending approval. `status` is the terminal bucket the
    /// caller resolved to — `"approved"`/`"denied"` for the binary card,
    /// `"decided"` for a chosen multi-option (#299), `"denied"` for
    /// Cancel. `value` is the chosen option's `value` (multi-option only;
    /// NULL for binary and Cancel — those carry the outcome in `status`).
    /// Returns `true` iff this call performed the decision (`false` =
    /// stale: the row was already terminal).
    ///
    /// Mirrors the bot's live in-process decision write (the bot owns its
    /// own rusqlite handle so it can't call this directly); kept coherent
    /// so the store-side decision contract is unit-testable. `teamctl
    /// approve/deny` writes directly with a canned SQL UPDATE.
    #[allow(dead_code)]
    pub fn decide_approval(
        &self,
        id: i64,
        status: &str,
        decided_by: &str,
        value: Option<&str>,
        note: Option<&str>,
    ) -> Result<bool> {
        let conn = self.connection()?;
        let n = conn.execute(
            "UPDATE approvals SET status=?1, decided_at=?2, decided_by=?3, decision_value=?4, decision_note=?5
             WHERE id=?6 AND status='pending' AND expires_at > ?2",
            params![status, Self::now(), decided_by, value, note, id],
        )?;
        Ok(n > 0)
    }
}
