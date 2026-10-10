use super::*;

/// Insert rows for every project + agent so `list_team` has something to return.
pub fn register_all_public(compose: &Compose) -> Result<()> {
    register_scope(compose, None)
}

pub fn register_project_public(compose: &Compose, project: &str) -> Result<()> {
    register_scope(compose, Some(project))
}

pub(super) fn register_scope(compose: &Compose, project: Option<&str>) -> Result<()> {
    use rusqlite::{params, Connection};
    let db = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    team_core::mailbox::ensure(&conn)?;
    let conn = conn.transaction()?;
    for p in compose
        .projects
        .iter()
        .filter(|p| project.is_none_or(|id| p.project.id == id))
    {
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET name=excluded.name",
            params![p.project.id, p.project.name],
        )?;
    }
    for h in compose
        .agents()
        .filter(|h| project.is_none_or(|id| h.project == id))
    {
        conn.execute(
            "INSERT INTO agents (id, project_id, role, runtime, is_manager, reports_to) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET role=excluded.role, runtime=excluded.runtime, is_manager=excluded.is_manager, reports_to=excluded.reports_to",
            params![
                h.id(),
                h.project,
                h.agent,
                h.spec.runtime,
                if h.is_manager { 1 } else { 0 },
                h.spec.reports_to.as_deref(),
            ],
        )?;
        // Per-agent ACLs.
        let can_dm = serde_json::to_string(&h.spec.can_dm)?;
        let can_bc = serde_json::to_string(&h.spec.can_broadcast)?;
        conn.execute(
            "INSERT INTO agent_acls (agent_id, can_dm_json, can_bcast_json)
             VALUES (?1,?2,?3)
             ON CONFLICT(agent_id) DO UPDATE SET can_dm_json=excluded.can_dm_json, can_bcast_json=excluded.can_bcast_json",
            params![h.id(), can_dm, can_bc],
        )?;
    }

    let desired: BTreeSet<_> = compose.agents().map(|h| h.id()).collect();
    let registered_agents: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT id FROM agents WHERE (?1 IS NULL OR project_id = ?1)")?;
        let rows = stmt.query_map(params![project], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for id in registered_agents
        .into_iter()
        .filter(|id| !desired.contains(id))
    {
        // Messages and receipts retain string identities independently of
        // the live roster; deleting stale agents cascades only their ACLs.
        conn.execute("DELETE FROM agents WHERE id = ?1", params![id])?;
    }
    if project.is_none() {
        let desired_projects: BTreeSet<_> = compose
            .projects
            .iter()
            .map(|p| p.project.id.as_str())
            .collect();
        let registered: Vec<String> = {
            let mut stmt = conn.prepare("SELECT id FROM projects")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in registered
            .into_iter()
            .filter(|id| !desired_projects.contains(id.as_str()))
        {
            conn.execute("DELETE FROM channels WHERE project_id = ?1", params![id])?;
            conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        }
    }

    // Channels + membership. Wipe and rewrite so removed members disappear.
    for p in compose
        .projects
        .iter()
        .filter(|p| project.is_none_or(|id| p.project.id == id))
    {
        // Channels carry live routing state. Remove dropped channels and
        // rebuild memberships atomically, preserving historical messages,
        // receipts, and agent identities referenced by those messages.
        conn.execute("DELETE FROM channel_members WHERE channel_id IN (SELECT id FROM channels WHERE project_id = ?1)", params![p.project.id])?;
        conn.execute(
            "DELETE FROM channels WHERE project_id = ?1",
            params![p.project.id],
        )?;
        for ch in &p.channels {
            let cid = format!("{}:{}", p.project.id, ch.name);
            let wildcard = matches!(
                ch.members,
                team_core::compose::ChannelMembers::All(ref s) if s == "*"
            );
            conn.execute(
                "INSERT INTO channels (id, project_id, name, wildcard) VALUES (?1,?2,?3,?4)
                 ON CONFLICT(id) DO UPDATE SET wildcard=excluded.wildcard",
                params![cid, p.project.id, ch.name, if wildcard { 1 } else { 0 }],
            )?;
            conn.execute(
                "DELETE FROM channel_members WHERE channel_id = ?1",
                params![cid],
            )?;
            match &ch.members {
                team_core::compose::ChannelMembers::All(_) => {
                    // Wildcard: join every agent in this project.
                    let agents: Vec<String> = p
                        .managers
                        .keys()
                        .chain(p.workers.keys())
                        .map(|a| format!("{}:{}", p.project.id, a))
                        .collect();
                    for aid in agents {
                        conn.execute(
                            "INSERT OR IGNORE INTO channel_members (channel_id, agent_id) VALUES (?1,?2)",
                            params![cid, aid],
                        )?;
                    }
                }
                team_core::compose::ChannelMembers::Explicit(members) => {
                    for m in members {
                        let aid = format!("{}:{}", p.project.id, m);
                        conn.execute(
                            "INSERT OR IGNORE INTO channel_members (channel_id, agent_id) VALUES (?1,?2)",
                            params![cid, aid],
                        )?;
                    }
                }
            }
        }
    }
    conn.commit()?;
    Ok(())
}
