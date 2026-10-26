use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::Store;

impl Store {
    /// Return every agent in the caller's project. Used by `list_team`.
    pub fn list_project_agents(&self, project: &str) -> Result<Vec<String>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare("SELECT id FROM agents WHERE project_id = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map(params![project], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return the project's org chart: managers (top tier) and per-worker
    /// `reports_to` links.
    pub fn org_chart(&self, project: &str) -> Result<serde_json::Value> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, runtime, is_manager, reports_to FROM agents WHERE project_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)? == 1,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let out = serde_json::json!({
            "project": project,
            "agents": rows.iter().map(|r| serde_json::json!({
                "id":r.0,"role":r.1,"runtime":r.2,"reports_to":r.4
            })).collect::<Vec<_>>()
        });
        Ok(out)
    }

    /// Upsert project+agent registration rows. Idempotent.
    /// Consumed by `teamctl up` (Chunk C); keep pub even though `team-mcp`
    /// itself doesn't call it.
    #[allow(dead_code)]
    pub fn upsert_agent(
        &self,
        agent_id: &str,
        project_id: &str,
        project_name: &str,
        role: &str,
        runtime: &str,
        is_manager: bool,
    ) -> Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name) VALUES (?1, ?2)",
            params![project_id, project_name],
        )?;
        conn.execute(
            "INSERT INTO agents (id, project_id, role, runtime, is_manager)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               role = excluded.role,
               runtime = excluded.runtime,
               is_manager = excluded.is_manager",
            params![
                agent_id,
                project_id,
                role,
                runtime,
                if is_manager { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    /// Is `agent_id` registered as a manager (`is_manager = 1`)? Used to
    /// gate `reply_to_user` so only managers can talk back to the human.
    pub fn is_manager(&self, agent_id: &str) -> Result<bool> {
        let conn = self.connection()?;
        let n: i64 = conn
            .query_row(
                "SELECT is_manager FROM agents WHERE id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(n == 1)
    }

    /// Runtime adapter registered for `agent_id` (e.g. `claude-code`,
    /// `codex`). Returns `None` if the agent isn't registered. Used by
    /// `compact_self` to gate the `/compact` slash command on Claude
    /// Code, since other runtimes don't recognize that command shape.
    pub fn runtime_for(&self, agent_id: &str) -> Result<Option<String>> {
        let conn = self.connection()?;
        let runtime = conn
            .query_row(
                "SELECT runtime FROM agents WHERE id = ?1",
                params![agent_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(runtime)
    }
}
