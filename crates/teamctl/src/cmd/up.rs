use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use team_core::compose::Compose;
use team_core::render::{env_path, mcp_path, render_agent};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

use crate::claude_trust;

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
    ensure_claude_trust(&compose)?;

    let sup = TmuxSupervisor;
    for h in compose.agents() {
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        sup.up(&spec)?;
        println!("up · {}", h.id());
    }

    // Spawn one team-bot per manager that carries a `telegram:` block.
    // Each bot runs in its own tmux session and is scoped via
    // --manager so DMs reach exactly that manager.
    let team_bot = super::bot::team_bot_bin();
    source_dotenv_into_process(&compose.root);
    for spec in super::bot::bot_specs(&compose) {
        match super::bot::up_one(&spec, &team_bot, &compose.root) {
            Ok(true) => println!("up · bot {} → {}", spec.session, spec.manager),
            Ok(false) => {}
            Err(e) => eprintln!("warn · bot {}: {e:#}", spec.session),
        }
    }

    // Persist the applied-state snapshot so a reload immediately
    // afterwards correctly sees zero diff. Before this, `up` left
    // `state/applied.json` absent, and the first reload misreported
    // every agent as `added`.
    let bin = super::team_mcp_bin().display().to_string();
    let snap = super::snapshot::compute(&compose, &bin);
    super::snapshot::write(&compose.root, &snap)?;
    Ok(())
}

/// Pre-accept Claude Code's per-workspace trust dialog for every cwd
/// that will host a `claude-code` agent. The trust-write itself lives in
/// `crate::claude_trust`; this function only collects the per-agent cwds.
fn ensure_claude_trust(compose: &Compose) -> Result<()> {
    let cwds: BTreeSet<PathBuf> = compose
        .agents()
        .filter(|h| h.spec.runtime == "claude-code")
        .filter_map(|h| {
            let project = compose
                .projects
                .iter()
                .find(|p| p.project.id == h.project)?;
            let cwd = if project.project.cwd.is_absolute() {
                project.project.cwd.clone()
            } else {
                compose.root.join(&project.project.cwd)
            };
            cwd.canonicalize().ok().or(Some(cwd))
        })
        .collect();
    claude_trust::pre_trust_cwds(&cwds)
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
    team_core::mailbox::ensure(&conn)?;
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

/// Write `bin/agent-wrapper.sh` and create `state/` subdirs.
///
/// The wrapper is teamctl-managed infrastructure: it gets rewritten on
/// every `teamctl up` so upgrading the binary picks up wrapper fixes
/// (pty handling, argv quoting, ...) without users having to rm and
/// re-init their workspace. Customization happens through env vars in
/// the generated `state/envs/<agent>.env`, not by editing the wrapper.
pub fn ensure_wrapper_and_dirs(compose: &Compose) -> Result<()> {
    let wrapper = super::agent_wrapper(&compose.root);
    if let Some(parent) = wrapper.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_write = match fs::read_to_string(&wrapper) {
        Ok(existing) => existing != DEFAULT_WRAPPER,
        Err(_) => true,
    };
    if needs_write {
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

const DEFAULT_WRAPPER: &str = include_str!("../../assets/agent-wrapper.sh");

/// Pull `<root>/.env` (and `<root>/../.env`) into the process so the
/// tmux session for `team-bot` inherits the bot token + chat-ids the
/// operator wrote with `teamctl bot setup`. Mirrors the loader in
/// `cmd::env::run`. Idempotent — never overwrites a value already in
/// the environment.
fn source_dotenv_into_process(root: &std::path::Path) {
    for f in [
        root.join(".env"),
        root.parent().unwrap_or(root).join(".env"),
    ] {
        if !f.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&f) else {
            continue;
        };
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if std::env::var_os(k).is_none() {
                    // SAFETY: single-threaded CLI startup.
                    unsafe { std::env::set_var(k, v) };
                }
            }
        }
    }
}
