use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use team_core::compose::Compose;
use team_core::render::{env_path, mcp_path, render_agent, write_role_prompt_concat};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, project: Option<&str>) -> Result<()> {
    let compose = super::load(root)?;
    super::update_check::maybe_print_banner(&compose.root);
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        bail!("{} validation error(s) — fix before up", errs.len());
    }
    let scoped = project
        .map(|name| super::project_filter::resolve(&compose, name))
        .transpose()?;

    // Per T-133: scoped runs skip cross-project work — wrapper write,
    // DB-side projects/agents/acls/channels rewrite, snapshot rewrite
    // — because each of those clobbers state owned by *other*
    // projects. The unscoped path is unchanged.
    if scoped.is_none() {
        ensure_wrapper_and_dirs(&compose)?;
        render_all_public(&compose)?;
        register_all_public(&compose)?;
        ensure_claude_trust(&compose)?;
    } else {
        // Per-project work: re-render the named project's env+mcp
        // (operator may have edited them) and pre-accept Claude trust
        // for that project's cwds. Both are idempotent and project-
        // scoped on disk.
        render_project_public(&compose, scoped.as_deref().unwrap())?;
        ensure_claude_trust_for_project(&compose, scoped.as_deref().unwrap())?;
    }

    let mut touched = 0usize;
    let sup = TmuxSupervisor;
    for h in compose.agents() {
        if scoped.as_deref().is_some_and(|id| id != h.project) {
            continue;
        }
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        sup.up(&spec)?;
        println!("up · {}", h.id());
        touched += 1;
    }

    // Spawn one team-bot per manager that carries a `telegram:` block.
    // Each bot runs in its own tmux session and is scoped via
    // --manager so DMs reach exactly that manager.
    let team_bot = super::bot::team_bot_bin();
    source_dotenv_into_process(&compose.root);
    for spec in super::bot::bot_specs(&compose) {
        if scoped
            .as_deref()
            .is_some_and(|id| spec.manager.split_once(':').map(|(p, _)| p) != Some(id))
        {
            continue;
        }
        match super::bot::up_one(&spec, &team_bot, &compose.root) {
            Ok(true) => {
                println!("up · bot {} → {}", spec.session, spec.manager);
                touched += 1;
            }
            Ok(false) => {}
            Err(e) => eprintln!("warn · bot {}: {e:#}", spec.session),
        }
    }

    if let (Some(id), 0) = (scoped.as_deref(), touched) {
        println!("no agents in project {id}.");
    }

    // Persist the applied-state snapshot so a reload immediately
    // afterwards correctly sees zero diff. Scoped runs merge just the
    // named project's per-agent entries into the existing
    // applied.json (T-133) — preserves correctness for the next
    // unscoped reload while still recording what this scoped up
    // applied.
    let bin = super::team_mcp_bin().display().to_string();
    let next = super::snapshot::compute(&compose, &bin);
    let snap = match scoped.as_deref() {
        Some(id) => {
            let prev = super::snapshot::read(&compose.root);
            super::snapshot::merge_project_into(prev.as_ref(), &next, id)
        }
        None => next,
    };
    super::snapshot::write(&compose.root, &snap)?;
    Ok(())
}

/// Render env + MCP for the named project's agents only. Mirrors
/// `render_all_public` but only iterates that project's agents. The
/// unscoped path remains the canonical "ensure dirs and render every
/// project" call.
pub fn render_project_public(compose: &Compose, project_id: &str) -> Result<()> {
    let envs_dir = compose.root.join("state/envs");
    let mcp_dir = compose.root.join("state/mcp");
    fs::create_dir_all(&envs_dir)?;
    fs::create_dir_all(&mcp_dir)?;
    let bin = super::team_mcp_bin().display().to_string();
    for h in compose.agents().filter(|h| h.project == project_id) {
        let (env, mcp) = render_agent(compose, h, &bin);
        fs::write(env_path(&compose.root, h.project, h.agent), env)?;
        fs::write(mcp_path(&compose.root, h.project, h.agent), mcp)?;
        // Mirror render_all_public: the scoped path must also
        // re-materialize multi-file role_prompt concat or a scoped
        // reload after a source-file edit boots the agent against a
        // stale concat file (zombie-prompt regression).
        write_role_prompt_concat(compose, h)
            .with_context(|| format!("write role_prompt concat for {}:{}", h.project, h.agent))?;
    }
    Ok(())
}

/// Pre-accept Claude Code's per-workspace trust dialog for every cwd that
/// will host a `claude-code` agent. Without this, the runtime blocks on a
/// "Do you trust this folder?" prompt the moment it boots, defeating the
/// "agents start working when teamctl up runs" model.
///
/// Running `teamctl up` is itself an explicit "I trust this directory"
/// signal -- the user is about to launch AI agents with tool access in
/// it -- so we record that consent in `~/.claude.json` once instead of
/// making them click through the dialog every restart.
fn ensure_claude_trust(compose: &Compose) -> Result<()> {
    ensure_claude_trust_inner(compose, None)
}

fn ensure_claude_trust_for_project(compose: &Compose, project_id: &str) -> Result<()> {
    ensure_claude_trust_inner(compose, Some(project_id))
}

fn ensure_claude_trust_inner(compose: &Compose, project_id: Option<&str>) -> Result<()> {
    let cwds: BTreeSet<PathBuf> = compose
        .agents()
        .filter(|h| project_id.map_or(true, |id| h.project == id))
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

    if cwds.is_empty() {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    let config_path = home.join(".claude.json");

    let mut config: serde_json::Value = match fs::read_to_string(&config_path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if !config
        .get("projects")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        config["projects"] = serde_json::json!({});
    }
    let projects = config["projects"].as_object_mut().unwrap();

    let mut newly_trusted = Vec::new();
    for cwd in &cwds {
        let key = cwd.display().to_string();
        let entry = projects
            .entry(key.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let obj = entry.as_object_mut().unwrap();
        let already = matches!(
            obj.get("hasTrustDialogAccepted"),
            Some(serde_json::Value::Bool(true))
        );
        if !already {
            obj.insert(
                "hasTrustDialogAccepted".into(),
                serde_json::Value::Bool(true),
            );
            newly_trusted.push(key);
        }
    }

    if newly_trusted.is_empty() {
        return Ok(());
    }

    // Write atomically so a concurrent claude reader never sees a
    // half-written config.
    let tmp = config_path.with_extension("json.teamctl.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&config)?)?;
    fs::rename(&tmp, &config_path)?;

    for path in newly_trusted {
        eprintln!("trust · auto-accepted Claude Code workspace trust for {path}");
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
        // Re-materialize multi-file role_prompt concat unconditionally
        // so any edit to a source file flows into the agent's prompt at
        // the next render — single-form is a no-op (back-compat).
        write_role_prompt_concat(compose, h)
            .with_context(|| format!("write role_prompt concat for {}:{}", h.project, h.agent))?;
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

#[cfg(test)]
mod tests {
    use super::DEFAULT_WRAPPER;
    use super::*;
    use std::collections::BTreeMap;
    use team_core::compose::*;
    use team_core::render::role_prompt_concat_path;

    /// The wrapper's `auto_confirm_known_dialogs` watcher relies on a
    /// fixed set of dialog-header substrings. A silent edit that drops
    /// one of them would re-strand agents at boot or mid-shift, so
    /// pin them here.
    #[test]
    fn wrapper_auto_confirm_patterns_present() {
        for marker in [
            "Loading development channels",
            "Bypass Permissions mode",
            "Stop and wait for limit to reset",
            "auto_confirm_known_dialogs",
        ] {
            assert!(
                DEFAULT_WRAPPER.contains(marker),
                "DEFAULT_WRAPPER missing marker: {marker}",
            );
        }
    }

    /// T-174: the wrapper picks `--resume` vs `--session-id` by
    /// probing the on-disk session jsonl. A silent edit that drops
    /// the resume branch would re-trigger "Session ID is already in
    /// use" on the second `teamctl up` under claude 2.1.138+; an edit
    /// that drops the create branch would break first-launch.
    /// Pin both markers plus the glob shape so neither half regresses.
    #[test]
    fn wrapper_session_id_resume_branch_present() {
        for marker in [
            "--session-id \"$CLAUDE_SESSION_ID\"",
            "--resume \"$CLAUDE_SESSION_ID\"",
            "$HOME/.claude/projects/",
            "$CLAUDE_SESSION_ID.jsonl",
        ] {
            assert!(
                DEFAULT_WRAPPER.contains(marker),
                "DEFAULT_WRAPPER missing marker: {marker}",
            );
        }
    }

    fn compose_with_multi_role_prompt(root: &Path, project_id: &str) -> Compose {
        let mut managers = BTreeMap::new();
        managers.insert(
            "mgr".into(),
            Agent {
                runtime: "claude-code".into(),
                model: None,
                role_prompt: Some(RolePrompt::Multiple(vec![
                    PathBuf::from("roles/_base.md"),
                    PathBuf::from("roles/mgr.md"),
                ])),
                permission_mode: None,
                autonomy: "low_risk_only".into(),
                can_dm: vec![],
                can_broadcast: vec![],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
                interfaces: None,
                display_name: None,
            },
        );
        Compose {
            root: root.to_path_buf(),
            global: Global {
                version: 2,
                broker: Default::default(),
                supervisor: Default::default(),
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
                attachments: Default::default(),
            },
            projects: vec![Project {
                version: 2,
                project: ProjectMeta {
                    id: project_id.into(),
                    name: project_id.into(),
                    cwd: root.to_path_buf(),
                },
                channels: vec![],
                managers,
                workers: Default::default(),
            }],
        }
    }

    #[test]
    fn render_project_public_writes_role_prompt_concat() {
        // Regression for T-103 qa finding: the scoped reload path
        // must materialize the multi-file role_prompt concat too,
        // else editing a source file and running
        // `teamctl reload <project>` boots the agent against a stale
        // concat file (zombie-prompt).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::write(root.join("roles/_base.md"), "BASE").unwrap();
        std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

        let compose = compose_with_multi_role_prompt(root, "p");
        render_project_public(&compose, "p").expect("render_project_public");

        let concat = role_prompt_concat_path(root, "p", "mgr");
        let got = std::fs::read_to_string(&concat).expect("concat file written");
        assert_eq!(got, "BASE\n\n—\n\nMGR");

        // No zombies: a source edit + re-render must update the concat.
        std::fs::write(root.join("roles/_base.md"), "BASE-v2").unwrap();
        render_project_public(&compose, "p").expect("render_project_public re-run");
        let got = std::fs::read_to_string(&concat).unwrap();
        assert_eq!(got, "BASE-v2\n\n—\n\nMGR");
    }
}
