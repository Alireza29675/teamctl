//! Approvals — the conditional stripe + the `a`-key modal.
//!
//! Two abstractions live here:
//!
//! - `ApprovalSource` — the read side. Returns the current set of
//!   pending `request_approval` rows for the operator to triage.
//!   Production impl `BrokerApprovalSource` queries SQLite; tests
//!   use `MockApprovalSource`.
//! - `ApprovalDecider` — the write side. Routes Approve / Reject
//!   through the existing `teamctl approve|deny` CLI so the
//!   T-031 `delivered_at` contract stays honored (the CLI flips
//!   `delivered_at` if it was null before recording the decision —
//!   see `crates/teamctl/src/cmd/approval.rs::decide`). Tests inject
//!   a `MockApprovalDecider` that records the calls.
//!
//! The CLI-routed write path is load-bearing: a direct SQLite
//! `UPDATE approvals SET status='approved' WHERE id=…` from the UI
//! would silently break the lifecycle invariant T-031 ships.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: i64,
    pub project_id: String,
    pub agent_id: String,
    pub action: String,
    pub summary: String,
    /// Optional free-form payload — for now the modal just shows
    /// the JSON if non-empty. PR-UI-4 doesn't try to pretty-print
    /// the diff shape.
    pub payload_json: String,
}

pub trait ApprovalSource: Send + Sync {
    /// Snapshot of every approval still in `status='pending'`.
    /// Empty vec when none. Errors fall back to empty in callers
    /// (the stripe just stays hidden).
    fn pending(&self) -> Result<Vec<Approval>>;
}

#[derive(Debug, Clone)]
pub struct BrokerApprovalSource {
    pub db_path: PathBuf,
}

impl BrokerApprovalSource {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl ApprovalSource for BrokerApprovalSource {
    fn pending(&self) -> Result<Vec<Approval>> {
        if !self.db_path.is_file() {
            return Ok(Vec::new());
        }
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_id, action, summary, payload_json FROM approvals
             WHERE status = 'pending'
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Approval {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    agent_id: r.get(2)?,
                    action: r.get(3)?,
                    summary: r.get(4)?,
                    payload_json: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                })
            })?
            .flatten()
            .collect();
        Ok(rows)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
}

pub trait ApprovalDecider: Send + Sync {
    /// Approve or deny the row at `id`, optionally with a note.
    /// Production impl shells out to the `teamctl` CLI so the
    /// T-031 `delivered_at` flip rides for free; tests inject a
    /// recorder.
    fn decide(&self, root: &std::path::Path, id: i64, kind: Decision, note: &str) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CliApprovalDecider;

impl ApprovalDecider for CliApprovalDecider {
    fn decide(&self, root: &std::path::Path, id: i64, kind: Decision, note: &str) -> Result<()> {
        let verb = match kind {
            Decision::Approve => "approve",
            Decision::Deny => "deny",
        };
        let mut cmd = Command::new("teamctl");
        cmd.arg("--root").arg(root).arg(verb).arg(id.to_string());
        if !note.is_empty() {
            cmd.arg("--note").arg(note);
        }
        let status = cmd
            .status()
            .with_context(|| format!("invoke teamctl {verb} {id}"))?;
        if !status.success() {
            anyhow::bail!("teamctl {verb} {id} exited {status}");
        }
        Ok(())
    }
}

pub mod test_support {
    //! Shared mocks — public so unit tests, integration tests, and
    //! downstream coverage can wire them in without rolling their own.

    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockApprovalSource {
        pub rows: Mutex<Vec<Approval>>,
    }

    impl MockApprovalSource {
        pub fn new(rows: Vec<Approval>) -> Self {
            Self {
                rows: Mutex::new(rows),
            }
        }
        pub fn set(&self, rows: Vec<Approval>) {
            *self.rows.lock().unwrap() = rows;
        }
    }

    impl ApprovalSource for MockApprovalSource {
        fn pending(&self) -> Result<Vec<Approval>> {
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    pub struct MockApprovalDecider {
        pub calls: Mutex<Vec<(i64, Decision, String)>>,
    }

    impl ApprovalDecider for MockApprovalDecider {
        fn decide(
            &self,
            _root: &std::path::Path,
            id: i64,
            kind: Decision,
            note: &str,
        ) -> Result<()> {
            self.calls.lock().unwrap().push((id, kind, note.into()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn ap(id: i64, action: &str, summary: &str) -> Approval {
        Approval {
            id,
            project_id: "p".into(),
            agent_id: "p:m".into(),
            action: action.into(),
            summary: summary.into(),
            payload_json: String::new(),
        }
    }

    #[test]
    fn mock_source_returns_what_it_was_seeded_with() {
        let src = MockApprovalSource::new(vec![ap(1, "publish", "post the brief")]);
        let rows = src.pending().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "publish");
    }

    #[test]
    fn mock_decider_records_calls() {
        let dec = MockApprovalDecider::default();
        dec.decide(std::path::Path::new("/x"), 7, Decision::Approve, "ship it")
            .unwrap();
        dec.decide(std::path::Path::new("/x"), 7, Decision::Deny, "")
            .unwrap();
        let calls = dec.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                (7, Decision::Approve, "ship it".to_string()),
                (7, Decision::Deny, String::new()),
            ]
        );
    }
}
