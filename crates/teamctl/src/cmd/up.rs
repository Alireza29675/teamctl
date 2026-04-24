use std::fs;
use std::path::Path;

use anyhow::{bail, Result};
use team_core::compose::Compose;
use team_core::render::{env_path, mcp_path, render_agent};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        bail!("{} validation error(s) — fix before up", errs.len());
    }
    ensure_wrapper_and_dirs(&compose)?;
    render_all_public(&compose)?;
    register_all_public(&compose)?;

    let sup = TmuxSupervisor;
    for h in compose.agents() {
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        sup.up(&spec)?;
        println!("up · {}", h.id());
    }
    Ok(())
}

/// Render per-agent env + MCP files. Called by `up` and `reload`.
pub fn render_all_public(compose: &Compose) -> Result<()> {
    let envs_dir = compose.root.join("state/envs");
    let mcp_dir = compose.root.join("state/mcp");
    fs::create_dir_all(&envs_dir)?;
    fs::create_dir_all(&mcp_dir)?;
    let bin = super::team_mcp_bin().display().to_string();
    for h in compose.agents() {
        let (env, mcp) = render_agent(compose, h, &bin);
        fs::write(env_path(&compose.root, h.project, h.agent), env)?;
        fs::write(mcp_path(&compose.root, h.project, h.agent), mcp)?;
    }
    Ok(())
}

/// Insert rows for every project + agent so `list_team` has something to return.
pub fn register_all_public(compose: &Compose) -> Result<()> {
    use rusqlite::{params, Connection};
    let db = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(team_core::mailbox::SCHEMA)?;
    for p in &compose.projects {
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name) VALUES (?1, ?2)",
            params![p.project.id, p.project.name],
        )?;
    }
    for h in compose.agents() {
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
        // Per-agent ACLs (Phase 2).
        let can_dm = serde_json::to_string(&h.spec.can_dm)?;
        let can_bc = serde_json::to_string(&h.spec.can_broadcast)?;
        conn.execute(
            "INSERT INTO agent_acls (agent_id, can_dm_json, can_bcast_json)
             VALUES (?1,?2,?3)
             ON CONFLICT(agent_id) DO UPDATE SET can_dm_json=excluded.can_dm_json, can_bcast_json=excluded.can_bcast_json",
            params![h.id(), can_dm, can_bc],
        )?;
    }

    // Channels + membership. Wipe and rewrite so removed members disappear.
    for p in &compose.projects {
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
    Ok(())
}

/// Write `bin/agent-wrapper.sh` and create `state/` subdirs if missing.
pub fn ensure_wrapper_and_dirs(compose: &Compose) -> Result<()> {
    let wrapper = super::agent_wrapper(&compose.root);
    if !wrapper.exists() {
        if let Some(parent) = wrapper.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&wrapper, DEFAULT_WRAPPER)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&wrapper)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&wrapper, perms)?;
    }
    fs::create_dir_all(compose.root.join("state/envs"))?;
    fs::create_dir_all(compose.root.join("state/mcp"))?;
    Ok(())
}

const DEFAULT_WRAPPER: &str = include_str!("../../../../bin/agent-wrapper.sh");
