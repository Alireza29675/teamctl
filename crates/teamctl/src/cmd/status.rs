use std::path::Path;

use anyhow::Result;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let sup = TmuxSupervisor;
    let depth = inbox_depths(&compose)?;

    println!(
        "{:<28} {:<10} {:<14} {:<18} INBOX",
        "AGENT", "MANAGER", "STATE", "TMUX",
    );
    for h in compose.agents() {
        let spec = AgentSpec::from_handle(h, &compose);
        let state = match sup.state(&spec).unwrap_or(AgentState::Unknown) {
            AgentState::Running => "running",
            AgentState::Stopped => "stopped",
            AgentState::Unknown => "unknown",
        };
        let n = depth.get(&h.id()).copied().unwrap_or(0);
        println!(
            "{:<28} {:<10} {:<14} {:<18} {}",
            h.id(),
            if h.is_manager { "yes" } else { "" },
            state,
            spec.tmux_session,
            n,
        );
    }
    Ok(())
}

fn inbox_depths(
    compose: &team_core::compose::Compose,
) -> Result<std::collections::BTreeMap<String, i64>> {
    use rusqlite::Connection;
    let mut map = std::collections::BTreeMap::new();
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        return Ok(map);
    }
    let conn = Connection::open(&db)?;
    for h in compose.agents() {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND acked_at IS NULL",
                rusqlite::params![h.id()],
                |r| r.get(0),
            )
            .unwrap_or(0);
        map.insert(h.id(), n);
    }
    Ok(map)
}
