//! `teamctl inspect <agent>` — full snapshot of one agent.

use std::path::Path;

use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use team_core::render::{env_path, mcp_path};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, target: &str) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };

    println!("# {target}");
    println!();
    println!("project:    {}", handle.project);
    println!("agent:      {}", handle.agent);
    println!(
        "role:       {}",
        if handle.is_manager {
            "manager"
        } else {
            "worker"
        }
    );
    println!("runtime:    {}", handle.spec.runtime);
    if let Some(m) = &handle.spec.model {
        println!("model:      {m}");
    }
    if let Some(p) = &handle.spec.role_prompt {
        println!("role file:  {}", compose.root.join(p).display());
    }
    if let Some(rt) = &handle.spec.reports_to {
        println!("reports to: {rt}");
    }
    println!("autonomy:   {}", handle.spec.autonomy);
    println!();

    let spec = AgentSpec::from_handle(
        handle,
        &compose.root,
        &compose.global.supervisor.tmux_prefix,
    );
    let state = TmuxSupervisor.state(&spec)?;
    println!("supervisor: {:?}", state);
    println!("tmux:       {}", spec.tmux_session);
    println!(
        "env file:   {}",
        env_path(&compose.root, handle.project, handle.agent).display()
    );
    println!(
        "mcp file:   {}",
        mcp_path(&compose.root, handle.project, handle.agent).display()
    );

    let db = compose.root.join(&compose.global.broker.path);
    if db.exists() {
        let conn = Connection::open(&db)?;

        println!();
        println!("## last 10 messages");
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text FROM messages
             WHERE sender = ?1 OR recipient = ?1
                OR recipient IN (
                    SELECT 'channel:' || cm.channel_id FROM channel_members cm WHERE cm.agent_id = ?1
                )
             ORDER BY id DESC LIMIT 10",
        )?;
        let mut rows = stmt.query(params![target])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let sender: String = r.get(1)?;
            let recipient: String = r.get(2)?;
            let text: String = r.get(3)?;
            let first = text.lines().next().unwrap_or("");
            println!("  #{id}  {sender}  →  {recipient}: {}", first);
        }

        println!();
        println!("## recent rate-limit hits");
        let mut stmt = conn.prepare(
            "SELECT id, hit_at, resets_at, raw_match FROM rate_limits
             WHERE agent_id = ?1 ORDER BY id DESC LIMIT 5",
        )?;
        let mut rows = stmt.query(params![target])?;
        let mut any = false;
        while let Some(r) = rows.next()? {
            any = true;
            let id: i64 = r.get(0)?;
            let raw: String = r.get(3)?;
            println!("  #{id}  {}", raw);
        }
        if !any {
            println!("  (none)");
        }
    }
    Ok(())
}
