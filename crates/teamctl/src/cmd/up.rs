use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use team_core::compose::Compose;
use team_core::render::{
    boot_script_path, claude_settings_path, env_path, mcp_path, render_agent,
    render_claude_settings, write_agent_skills, write_role_prompt_concat, write_subagents_json,
};
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

use super::agent_filter::AgentSelector;

pub fn run(root: &Path, project: Option<&str>, sel: &AgentSelector, fresh: bool) -> Result<()> {
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
    // Per-agent target set (T-305). `None` => no agent-level filter
    // (no-arg / `<project>`-only contracts, untouched). Only reached
    // when `scoped` is `Some` — clap requires a project for the
    // selector forms.
    let targets = match scoped.as_deref() {
        Some(id) => super::agent_filter::resolve(&compose, id, sel)?,
        None => None,
    };

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
        if targets.as_ref().is_some_and(|t| !t.contains(h.agent)) {
            continue;
        }
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        let running = matches!(sup.state(&spec)?, AgentState::Running);
        // In a per-agent scope the operator named this agent on the
        // command line, so an already-running session is worth calling
        // out explicitly rather than silently. `up` stays idempotent
        // (sup.up() is a no-op for a running session) — this only adds
        // a clearer line, never an error.
        if targets.is_some() && running {
            println!("up · {} (already running)", h.id());
            touched += 1;
            continue;
        }
        // Only `--fresh` an agent we're actually about to spawn. `up`
        // never restarts a running agent (sup.up() is a no-op), so
        // freshening a running one would move its live session aside with
        // no respawn to replace it — a latent desync where the agent
        // silently comes up fresh on its NEXT natural restart. Skip
        // running agents here; `reload --fresh` is the path to refresh a
        // running agent's conversation.
        if !running {
            freshen_for_spec(&spec, &h.spec.runtime, fresh);
        }
        sup.up(&spec)?;
        println!("up · {}{}", h.id(), fresh_suffix(fresh && !running));
        touched += 1;
    }

    // Spawn one team-bot per manager that carries a `telegram:` block.
    // Each bot runs in its own tmux session and is scoped via
    // --manager so DMs reach exactly that manager.
    let team_bot = super::bot::team_bot_bin();
    source_dotenv_into_process(&compose.root);
    for spec in super::bot::bot_specs(&compose) {
        // Project guard preserved verbatim from the pre-T-305 path.
        let split = spec.manager.split_once(':');
        if scoped
            .as_deref()
            .is_some_and(|id| split.map(|(p, _)| p) != Some(id))
        {
            continue;
        }
        // A bot's lifecycle follows its manager agent: in a per-agent
        // scope, skip the bot unless its manager is targeted.
        // `targets` is `Some` only when `scoped` is `Some`, so the
        // guard above has already pinned `split` to the in-scope pair.
        if let Some(t) = &targets {
            if !t.contains(split.map(|(_, a)| a).unwrap_or("")) {
                continue;
            }
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
        println!("no agents in scope for project {id}.");
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

    // T-370: keep the host awake (no idle-sleep) while agents are up so
    // long-running tasks survive display sleep. Host-level + refcounted;
    // macOS-only, no-op elsewhere. Only when we actually brought something up.
    if touched > 0 {
        super::caffeinate::ensure_running();
    }
    Ok(())
}

/// What a `--fresh` request resolves to for one agent, before any I/O.
/// Split out from [`freshen_for_spec`] so the runtime-gate decision —
/// the codex/gemini parity carve-out — is unit-testable without touching
/// the filesystem or `$HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshenAction {
    /// `--fresh` not set: do nothing.
    Skip,
    /// `--fresh` on a non-Claude runtime: warn and skip (parity gap).
    UnsupportedRuntime,
    /// `--fresh` on a Claude agent: move its session aside.
    Freshen,
}

/// Resolve `(runtime, fresh)` to a [`FreshenAction`]. Pure — no I/O.
pub(crate) fn freshen_action(runtime: &str, fresh: bool) -> FreshenAction {
    if !fresh {
        FreshenAction::Skip
    } else if runtime == "claude-code" {
        FreshenAction::Freshen
    } else {
        FreshenAction::UnsupportedRuntime
    }
}

/// T-352: when `--fresh` is set, move the agent's Claude session JSONL
/// aside just before it (re)spawns so the wrapper opens a brand-new
/// conversation at the same deterministic UUID (re-running
/// `BOOTSTRAP_PROMPT`). Durable on-disk files are never touched.
///
/// Claude runtime only: codex/gemini have different (or no) session
/// resume, so we warn-and-skip rather than abort a mixed-runtime team
/// (parity gap, v1). Best-effort — a move failure warns but never blocks
/// the respawn (coming up on the existing conversation is strictly safer
/// than refusing to start).
///
/// Call only for an agent that is actually being (re)spawned: freshening
/// an agent that won't respawn would move its live session aside with no
/// new conversation to replace it (a latent desync), so the callers gate
/// on "about to start this agent" before calling here.
pub(crate) fn freshen_for_spec(spec: &AgentSpec, runtime: &str, fresh: bool) {
    let id = format!("{}:{}", spec.project, spec.agent);
    match freshen_action(runtime, fresh) {
        FreshenAction::Skip => {}
        FreshenAction::UnsupportedRuntime => {
            eprintln!("warn · {id} (--fresh skipped: {runtime} runtime has no session resume yet)");
        }
        FreshenAction::Freshen => {
            let Some(home) = team_core::session::claude_home() else {
                eprintln!("warn · {id} (--fresh skipped: $HOME unset)");
                return;
            };
            if let Err(e) = team_core::session::freshen_session(&home, &spec.project, &spec.agent) {
                eprintln!("warn · {id} (--fresh: could not move session aside: {e})");
            }
        }
    }
}

/// `" (fresh)"` when a `--fresh` restart is in effect, else empty. Kept a
/// free function so `up` and `reload` annotate their per-line logs and
/// dry-run output identically.
pub(crate) fn fresh_suffix(fresh: bool) -> &'static str {
    if fresh {
        " (fresh)"
    } else {
        ""
    }
}

/// Render env + MCP for the named project's agents only. Mirrors
/// `render_all_public` but only iterates that project's agents. The
/// unscoped path remains the canonical "ensure dirs and render every
/// project" call.
pub fn render_project_public(compose: &Compose, project_id: &str) -> Result<()> {
    let envs_dir = compose.root.join("state/envs");
    let mcp_dir = compose.root.join("state/mcp");
    let claude_dir = compose.root.join("state/claude");
    fs::create_dir_all(&envs_dir)?;
    fs::create_dir_all(&mcp_dir)?;
    fs::create_dir_all(&claude_dir)?;
    // #428: per-agent activity-heartbeat markers (touched by the rendered
    // hooks, stat()d by the TUI). Created here alongside the other state
    // subdirs so the rendered hook can be a bare `touch` of the marker.
    fs::create_dir_all(compose.root.join("state/heartbeats"))?;
    let bin = super::team_mcp_bin().display().to_string();
    for h in compose.agents().filter(|h| h.project == project_id) {
        let (env, mcp) = render_agent(compose, h, &bin);
        fs::write(env_path(&compose.root, h.project, h.agent), env)?;
        fs::write(mcp_path(&compose.root, h.project, h.agent), mcp)?;
        if let Some(settings) = render_claude_settings(compose, h) {
            fs::write(
                claude_settings_path(&compose.root, h.project, h.agent),
                settings,
            )?;
        }
        // Mirror render_all_public: the scoped path must also
        // re-materialize multi-file role_prompt concat or a scoped
        // reload after a source-file edit boots the agent against a
        // stale concat file (zombie-prompt regression).
        write_role_prompt_concat(compose, h)
            .with_context(|| format!("write role_prompt concat for {}:{}", h.project, h.agent))?;
        // #383 Phase 3a: render the per-agent `--agents` JSON (or clear a
        // stale one) alongside the env/mcp/settings files.
        write_subagents_json(compose, h)
            .with_context(|| format!("write sub-agents json for {}:{}", h.project, h.agent))?;
        // #383 Phase 3b: materialize (or clear) the per-agent skills scope
        // dir so `claude --add-dir` surfaces declared skills.
        write_agent_skills(compose, h)
            .with_context(|| format!("write agent skills for {}:{}", h.project, h.agent))?;
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
        .filter(|h| project_id.is_none_or(|id| h.project == id))
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

    // Transparency: editing the user's Claude Code config on their behalf is
    // not something we do silently. Tell them — at `up`, where a human is at
    // the terminal — exactly which folders we marked trusted and where, and
    // why. (This is a notice, not a prompt: agent panes stay non-interactive.)
    // User-facing copy: no em-dash (owner house style); singular/plural agree.
    // Wording is neda's polish pass (msg 1634).
    let n = newly_trusted.len();
    let folder_word = if n == 1 { "folder" } else { "folders" };
    let delete_phrase = if n == 1 { "that key" } else { "those keys" };
    eprintln!();
    eprintln!("trust · marked {n} {folder_word} trusted in your Claude Code config");
    eprintln!("        so agents don't stall on Claude's \"trust this folder\" prompt:");
    for path in &newly_trusted {
        eprintln!("          • {path}");
    }
    eprintln!(
        "        config: {} (key: hasTrustDialogAccepted)",
        config_path.display()
    );
    eprintln!("        running `teamctl up` granted this trust; delete {delete_phrase} to undo.");
    eprintln!();
    Ok(())
}

/// Render per-agent env + MCP files. Called by `up` and `reload`.
pub fn render_all_public(compose: &Compose) -> Result<()> {
    let envs_dir = compose.root.join("state/envs");
    let mcp_dir = compose.root.join("state/mcp");
    let claude_dir = compose.root.join("state/claude");
    fs::create_dir_all(&envs_dir)?;
    fs::create_dir_all(&mcp_dir)?;
    fs::create_dir_all(&claude_dir)?;
    // #428: per-agent activity-heartbeat markers (touched by the rendered
    // hooks, stat()d by the TUI). Created here alongside the other state
    // subdirs so the rendered hook can be a bare `touch` of the marker.
    fs::create_dir_all(compose.root.join("state/heartbeats"))?;
    let bin = super::team_mcp_bin().display().to_string();
    for h in compose.agents() {
        let (env, mcp) = render_agent(compose, h, &bin);
        fs::write(env_path(&compose.root, h.project, h.agent), env)?;
        fs::write(mcp_path(&compose.root, h.project, h.agent), mcp)?;
        if let Some(settings) = render_claude_settings(compose, h) {
            fs::write(
                claude_settings_path(&compose.root, h.project, h.agent),
                settings,
            )?;
        }
        // Re-materialize multi-file role_prompt concat unconditionally
        // so any edit to a source file flows into the agent's prompt at
        // the next render — single-form is a no-op (back-compat).
        write_role_prompt_concat(compose, h)
            .with_context(|| format!("write role_prompt concat for {}:{}", h.project, h.agent))?;
        // #383 Phase 3a: render the per-agent `--agents` JSON (or clear a
        // stale one) alongside the env/mcp/settings files.
        write_subagents_json(compose, h)
            .with_context(|| format!("write sub-agents json for {}:{}", h.project, h.agent))?;
        // #383 Phase 3b: materialize (or clear) the per-agent skills scope
        // dir so `claude --add-dir` surfaces declared skills.
        write_agent_skills(compose, h)
            .with_context(|| format!("write agent skills for {}:{}", h.project, h.agent))?;
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

/// Write `bin/agent-wrapper.sh`, `bin/boot.sh`, and create `state/` subdirs.
///
/// Both scripts are teamctl-managed infrastructure: they get rewritten on
/// every `teamctl up` so upgrading the binary picks up wrapper fixes (pty
/// handling, argv quoting, ...) and boot-context fixes without users having
/// to rm and re-init their workspace. Customization happens through env vars
/// in the generated `state/envs/<agent>.env`, not by editing the scripts.
pub fn ensure_wrapper_and_dirs(compose: &Compose) -> Result<()> {
    write_managed_executable(&super::agent_wrapper(&compose.root), DEFAULT_WRAPPER)?;
    write_managed_executable(&boot_script_path(&compose.root), DEFAULT_BOOT_SCRIPT)?;
    fs::create_dir_all(compose.root.join("state/envs"))?;
    fs::create_dir_all(compose.root.join("state/mcp"))?;
    Ok(())
}

/// Write a teamctl-managed executable asset and make it `0o755` on unix.
/// Idempotent: only rewrites when the on-disk copy has drifted from the
/// embedded one, so a `teamctl up` that changes nothing leaves mtimes alone.
fn write_managed_executable(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_write = match fs::read_to_string(path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if needs_write {
        fs::write(path, content)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

const DEFAULT_WRAPPER: &str = include_str!("../../assets/agent-wrapper.sh");
const DEFAULT_BOOT_SCRIPT: &str = include_str!("../../assets/boot.sh");

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
    use super::DEFAULT_BOOT_SCRIPT;
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
    fn freshen_action_gates_on_fresh_and_runtime() {
        // Watch-out (c): the codex/gemini parity carve-out. Not fresh →
        // nothing, regardless of runtime. Fresh → freshen only Claude;
        // every other runtime is a warn-and-skip, never an abort.
        assert_eq!(freshen_action("claude-code", false), FreshenAction::Skip);
        assert_eq!(freshen_action("codex", false), FreshenAction::Skip);
        assert_eq!(freshen_action("claude-code", true), FreshenAction::Freshen);
        assert_eq!(
            freshen_action("codex", true),
            FreshenAction::UnsupportedRuntime
        );
        assert_eq!(
            freshen_action("gemini", true),
            FreshenAction::UnsupportedRuntime
        );
    }

    /// The auto-confirm watcher dismisses the one-shot dialogs that would
    /// otherwise strand a headless pane. `Quick safety check:` is the
    /// first-run trust-folder prompt that `--permission-mode auto` (the
    /// headless default since 0.8.7) raises — the watcher didn't match it,
    /// so an auto session froze at boot whenever the pre-trust missed. It
    /// is a one-time trust gate, not `auto`'s risky-action classifier, so
    /// accepting it keeps the safety gate intact; the watcher must never
    /// match `auto`'s risky-action prompts. Because the header is ordinary
    /// prose, the watcher requires it to co-occur with the menu line
    /// `trust this folder` before sending Enter — pin both so a future edit
    /// can't drop the co-occurrence guard and reintroduce stray Enters. The
    /// MCP-enable dialog is auto-accepted the same way (Enter enables the
    /// discovered project MCP servers), gated on its own two-string
    /// co-occurrence so prose can't trip it.
    #[test]
    fn wrapper_auto_confirm_patterns_present() {
        for marker in [
            "Loading development channels",
            "Bypass Permissions mode",
            "Stop and wait for limit to reset",
            "Quick safety check:",
            "MCP servers may execute code",
            "auto_confirm_known_dialogs",
        ] {
            assert!(
                DEFAULT_WRAPPER.contains(marker),
                "DEFAULT_WRAPPER missing marker: {marker}",
            );
        }
        // The trust-folder and MCP-enable dialogs must each be gated on a
        // two-string co-occurrence, not a bare match: both greps per dialog
        // must be present in the watcher.
        assert!(
            DEFAULT_WRAPPER.contains("grep -q 'Quick safety check:'")
                && DEFAULT_WRAPPER.contains("grep -q 'trust this folder'"),
            "watcher must require 'Quick safety check:' AND 'trust this folder' to co-occur",
        );
        assert!(
            DEFAULT_WRAPPER.contains("grep -q 'MCP servers may execute code'")
                && DEFAULT_WRAPPER.contains("grep -q 'Enter to confirm · Esc'"),
            "watcher must require 'MCP servers may execute code' AND the 'Enter to confirm · Esc' \
             footer chrome to co-occur",
        );
    }

    /// #383 Phase 3a: the wrapper threads per-agent sub-agents via
    /// `--agents` when render wrote the JSON file (guarded by `[ -f ]`, so
    /// agents with no `subagents:` pass no flag). Pin the marker so a silent
    /// wrapper edit can't drop it and strand declared sub-agents.
    #[test]
    fn wrapper_threads_subagents_via_agents_flag() {
        assert!(
            DEFAULT_WRAPPER.contains("--agents \"$(cat \"$CLAUDE_AGENTS_JSON\")\""),
            "wrapper must pass --agents from CLAUDE_AGENTS_JSON",
        );
    }

    /// #383 Phase 3b: the wrapper threads per-agent skills via `--add-dir`
    /// when render materialized the scope dir (guarded by `[ -d ]`, so
    /// agents with no `skills:` pass no flag). Pin the marker so a silent
    /// wrapper edit can't drop it and strand declared skills.
    #[test]
    fn wrapper_threads_skills_via_add_dir_flag() {
        assert!(
            DEFAULT_WRAPPER.contains("--add-dir \"$CLAUDE_AGENT_SCOPE\""),
            "wrapper must pass --add-dir from CLAUDE_AGENT_SCOPE",
        );
    }

    /// T-361: headless claude-code agents default to `--permission-mode
    /// auto` and no longer pass `--dangerously-skip-permissions`. The
    /// attended opt-out keys off PERMISSION_MODE, which `render()` omits for
    /// agents with no `permission_mode:` — so under `set -u` the comparison
    /// must default the var. Pin the new shape so a silent edit can't bring
    /// back the bypass-everything flag, drop the auto default, or break the
    /// unset-safe attended branch.
    #[test]
    fn wrapper_defaults_headless_to_permission_mode_auto() {
        assert!(
            DEFAULT_WRAPPER.contains("--permission-mode \"${PERMISSION_MODE:-auto}\""),
            "wrapper must default headless agents to --permission-mode auto",
        );
        assert!(
            !DEFAULT_WRAPPER.contains("--dangerously-skip-permissions"),
            "wrapper must not pass --dangerously-skip-permissions (#361)",
        );
        assert!(
            DEFAULT_WRAPPER.contains("[ \"${PERMISSION_MODE:-}\" = \"attended\" ]"),
            "wrapper must keep the set -u-safe attended opt-out branch",
        );
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

    /// T-190: the wrapper runs under `set -u`. Both
    /// `CLAUDE_SESSION_ID` and `CLAUDE_SESSION_NAME` are rendered
    /// into the env file only for `runtime: claude-code` agents
    /// (team-core::render::render_env). If they're absent for any
    /// reason — env file from an older render, write race, future
    /// runtime variant — the unguarded `[ -n "$CLAUDE_SESSION_ID" ]`
    /// reference aborts the wrapper, the tmux pane closes, and the
    /// supervisor marks the agent stopped without a diagnostic.
    /// The defaults at the top of the wrapper close that hole; a
    /// silent edit that drops them re-opens the failure mode.
    #[test]
    fn wrapper_session_vars_have_set_u_defaults() {
        for marker in [
            ": \"${CLAUDE_SESSION_ID:=}\"",
            ": \"${CLAUDE_SESSION_NAME:=}\"",
        ] {
            assert!(
                DEFAULT_WRAPPER.contains(marker),
                "DEFAULT_WRAPPER missing marker: {marker}",
            );
        }
    }

    /// T-190: macOS ships bash 3.2 as `/bin/sh`. Bash 3.2 has a
    /// parser bug where `${VAR:=DEFAULT}` cannot reliably parse
    /// escape sequences inside DEFAULT (backslash-backtick,
    /// backslash-quote). The wrapper's BOOTSTRAP_PROMPT default
    /// contains both, so the pre-T-190 `${BOOTSTRAP_PROMPT:=...}`
    /// shape aborted every spawn on macOS — the 0.8.0 fresh-install
    /// regression. The fix is conditional assignment, not parameter
    /// expansion: pin that BOTH the `:=` form is GONE and the
    /// `[ -z ]` plain-assignment shape is present, so a future
    /// cleanup can't silently regress macOS again.
    #[test]
    fn wrapper_bootstrap_prompt_default_is_macos_safe() {
        assert!(
            !DEFAULT_WRAPPER.contains("${BOOTSTRAP_PROMPT:="),
            "DEFAULT_WRAPPER still uses ${{VAR:=DEFAULT}} for \
             BOOTSTRAP_PROMPT — that shape is bash-3.2-fatal on \
             macOS when DEFAULT contains escape sequences. Keep \
             the conditional-assignment form.",
        );
        for marker in [
            "if [ -z \"${BOOTSTRAP_PROMPT:-}\" ]; then",
            "BOOTSTRAP_PROMPT=\"Begin your shift as ${AGENT}.",
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
                ultracode: false,
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
                skills: vec![],
            },
        );
        Compose {
            root: root.to_path_buf(),
            global: Global {
                version: team_core::compose::SchemaVersion::new("2.0.0"),
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
                interfaces: None,
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

        // #428: render must create the heartbeat dir so the rendered bare
        // `touch` hook has somewhere to write. If this regresses, the dir is
        // missing => `touch` fails => no marker => every agent reads Idle
        // (the safe failure, but a silent loss of the working signal).
        assert!(
            root.join("state/heartbeats").is_dir(),
            "render must create state/heartbeats/"
        );

        let concat = role_prompt_concat_path(root, "p", "mgr");
        let got = std::fs::read_to_string(&concat).expect("concat file written");
        assert_eq!(got, "BASE\n\n—\n\nMGR");

        // No zombies: a source edit + re-render must update the concat.
        std::fs::write(root.join("roles/_base.md"), "BASE-v2").unwrap();
        render_project_public(&compose, "p").expect("render_project_public re-run");
        let got = std::fs::read_to_string(&concat).unwrap();
        assert_eq!(got, "BASE-v2\n\n—\n\nMGR");
    }

    /// Serializes the HOME-mutating test(s) in this binary; `$HOME` is
    /// process-global, so a concurrent reader/writer would race.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point `$HOME` at `home` for the guard's lifetime, then restore it, so
    /// the trust write lands in a throwaway `.claude.json`, never the real one.
    struct HomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("HOME");
            // SAFETY: HOME_LOCK serializes every HOME mutation in this binary,
            // matching the `unsafe { set_var }` convention elsewhere in up.rs.
            unsafe { std::env::set_var("HOME", home) };
            Self { _lock: lock, prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: still holding HOME_LOCK (see `set`).
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    /// `ensure_claude_trust` pre-accepts Claude's workspace-trust dialog by
    /// writing `hasTrustDialogAccepted: true` under each claude-code agent's
    /// cwd, and is a no-op on a second run (trust already on disk) so the `up`
    /// notice doesn't nag on every restart.
    #[test]
    fn ensure_claude_trust_writes_key_then_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".team");
        std::fs::create_dir_all(root.join("projects")).unwrap();
        std::fs::write(
            root.join("team-compose.yaml"),
            r#"
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
supervisor:
  type: tmux
  tmux_prefix: a-
projects:
  - file: projects/hello.yaml
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("projects/hello.yaml"),
            r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
"#,
        )
        .unwrap();
        let compose = Compose::load(&root).expect("compose loads");

        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let config_path = home.join(".claude.json");
        let _guard = HomeGuard::set(&home);

        // First run writes the trust key for the agent's (canonicalized) cwd.
        ensure_claude_trust(&compose).expect("first ensure_claude_trust");
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&config_path).expect("wrote .claude.json"),
        )
        .expect("config is valid json");
        let key = root.canonicalize().unwrap().display().to_string();
        assert_eq!(
            cfg["projects"][&key]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
            "trust key must be written for the agent cwd; config: {cfg}",
        );

        // Second run is a no-op: trust is on disk, so nothing is rewritten
        // (and the operator notice does not re-fire).
        let before = std::fs::read_to_string(&config_path).unwrap();
        ensure_claude_trust(&compose).expect("second ensure_claude_trust");
        let after = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(before, after, "second run must not rewrite the config");
    }

    /// #430: the boot-context asset must carry the REQUIRED `hookEventName`
    /// inside `hookSpecificOutput` — without it Claude Code silently drops
    /// `additionalContext` and the hook injects nothing while still exiting 0
    /// (the exact silent-no-op the de-risk pass caught). Pin that, plus the
    /// `SessionStart` event name and the wake-aware verb mapping, so a future
    /// edit can't quietly hollow the asset out.
    #[test]
    fn boot_script_emits_session_start_context() {
        assert!(
            DEFAULT_BOOT_SCRIPT.contains(r#""hookEventName":"SessionStart""#),
            "boot.sh must emit the required hookEventName or CC drops additionalContext"
        );
        assert!(
            DEFAULT_BOOT_SCRIPT.contains("additionalContext"),
            "boot.sh must emit additionalContext"
        );
        for verb in ["resumed", "cleared context", "compacted", "booted"] {
            assert!(
                DEFAULT_BOOT_SCRIPT.contains(verb),
                "boot.sh missing wake-aware verb: {verb}"
            );
        }
        // POSIX `/bin/sh` shebang + `set -u`, matching the wrapper's contract
        // (the command runs in the agent's shell, macOS bash 3.2 included).
        assert!(DEFAULT_BOOT_SCRIPT.starts_with("#!/bin/sh"));
        assert!(DEFAULT_BOOT_SCRIPT.contains("set -u"));
        // #439: the two source-specific extensions and the argv-optional
        // guards. The `${1:-}` / `${2:-}` reads keep the script `set -u`-safe
        // when an older rendered hook passes no argv (downtime then omits).
        assert!(
            DEFAULT_BOOT_SCRIPT.contains("${1:-}") && DEFAULT_BOOT_SCRIPT.contains("${2:-}"),
            "boot.sh must guard its optional argv under set -u"
        );
        assert!(
            DEFAULT_BOOT_SCRIPT.contains("You were down for"),
            "boot.sh must carry the downtime sentence"
        );
        assert!(
            DEFAULT_BOOT_SCRIPT.contains("Re-anchor before continuing"),
            "boot.sh must carry the compact re-anchor copy"
        );
    }

    /// #439: execute the real boot.sh asset on the host shell and assert the
    /// wake notice per `source` × file-state. Each run exercises only its
    /// host's native branch — macOS takes the BSD `stat -f`/`date -r` path,
    /// Linux the GNU `stat -c`/`date -d` path — so it is the CI matrix across
    /// BOTH the macos-14 and ubuntu-24.04 legs that proves the two portability
    /// fallbacks on real hardware, not just reasoned about. Mtimes are set
    /// precisely via `File::set_modified` (in std since 1.75, under our 1.86
    /// MSRV) so the buckets are deterministic.
    #[test]
    fn boot_script_reports_downtime_and_reanchor() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        use std::time::{Duration, SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("boot.sh");
        std::fs::write(&script, DEFAULT_BOOT_SCRIPT).unwrap();

        // Run boot.sh under /bin/sh with the given stdin source + argv paths,
        // returning the parsed `additionalContext`. Parsing as JSON also
        // enforces the ASCII no-escaping contract: a stray quote in any
        // injected copy would break this parse and fail the test.
        let run = |source: &str, args: &[&std::path::Path]| -> String {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg(&script);
            for a in args {
                cmd.arg(a);
            }
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = cmd.spawn().unwrap();
            let payload = format!("{{\"source\":\"{source}\"}}");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "boot.sh exited non-zero for source={source}"
            );
            let stdout = String::from_utf8(out.stdout).unwrap();
            let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .to_string()
        };

        // A file whose mtime is `secs_ago` seconds before now.
        let aged = |name: &str, secs_ago: u64| -> std::path::PathBuf {
            let p = dir.path().join(name);
            let f = std::fs::File::create(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
                .unwrap();
            p
        };
        let missing = dir.path().join("does-not-exist");

        // source variants, no argv: base notice only, with the compact
        // re-anchor appended on `compact`.
        let startup = run("startup", &[]);
        assert!(startup.starts_with("You booted at "), "{startup}");
        assert!(
            !startup.contains("You were down"),
            "no argv => no downtime: {startup}"
        );
        for (src, lead) in [
            ("resume", "You resumed at "),
            ("clear", "You cleared context at "),
        ] {
            let out = run(src, &[]);
            assert!(out.starts_with(lead), "{out}");
            assert!(
                !out.contains("You were down") && !out.contains("Re-anchor"),
                "{src} must stay the base notice: {out}"
            );
        }
        let compact = run("compact", &[]);
        assert!(compact.starts_with("You compacted at "), "{compact}");
        assert!(
            compact.contains("Re-anchor before continuing: re-read your working files"),
            "compact must carry the re-anchor copy: {compact}"
        );

        // startup downtime from LASTSEEN ($1), marker ($2) absent.
        assert!(
            run("startup", &[&aged("ls_2h", 7200), &missing])
                .contains("You were down for about 2 hours (last active "),
            "2h lastseen => 2 hours"
        );
        assert!(
            run("startup", &[&aged("ls_30s", 30), &missing])
                .contains("You were down for under a minute (last active "),
            "30s lastseen => under a minute"
        );

        // Unclean shutdown: the marker survived and is fresher than LASTSEEN,
        // so its mtime wins.
        assert!(
            run("startup", &[&aged("l_2h", 7200), &aged("m_10m", 600)])
                .contains("You were down for about 10 minutes "),
            "present marker (10m) beats lastseen (2h)"
        );

        // Omit cases: both files missing, and a non-startup source never
        // reports downtime even with a usable file present.
        assert!(
            !run("startup", &[&missing, &missing]).contains("You were down"),
            "both missing => omit"
        );
        assert!(
            !run("resume", &[&aged("ls_for_resume", 7200), &missing]).contains("You were down"),
            "resume must not report downtime"
        );
    }

    /// #430: `teamctl up` materializes `bin/boot.sh` next to the wrapper, with
    /// the embedded content and a 0o755 mode, so Claude Code's SessionStart
    /// hook can execute it. Mirrors the wrapper's managed-asset contract.
    #[test]
    fn ensure_wrapper_and_dirs_writes_executable_boot_script() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".team");
        std::fs::create_dir_all(root.join("projects")).unwrap();
        std::fs::write(
            root.join("team-compose.yaml"),
            r#"
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
supervisor:
  type: tmux
  tmux_prefix: a-
projects:
  - file: projects/hello.yaml
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("projects/hello.yaml"),
            r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
"#,
        )
        .unwrap();
        let compose = Compose::load(&root).expect("compose loads");

        ensure_wrapper_and_dirs(&compose).expect("ensure_wrapper_and_dirs");

        let boot = team_core::render::boot_script_path(&compose.root);
        assert!(boot.is_file(), "bin/boot.sh must be written");
        assert_eq!(
            std::fs::read_to_string(&boot).unwrap(),
            DEFAULT_BOOT_SCRIPT,
            "on-disk boot.sh must match the embedded asset"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&boot).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "boot.sh must be chmod 0o755");
        }
    }
}
