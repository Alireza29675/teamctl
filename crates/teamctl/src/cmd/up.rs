use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use team_core::compose::Compose;
use team_core::render::{codex_home_dir, opencode_home_dir};
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

    // T-469: refuse a same-name-different-folder launch before any side
    // effects. Two teams sharing a `project_id` in different folders alias
    // each other (identity is name-keyed, not path-keyed) and crash-loop;
    // bail early with the conflicting directory named.
    let _identity_lock = team_core::registry::lifecycle_lock()?;
    guard_no_name_collision(&compose, scoped.as_deref())?;

    let _lock = team_core::persistence::ProcessLock::acquire(
        &compose.root.join("state/lifecycle.lock.sqlite"),
    )?;
    source_dotenv_into_process(&compose.root);
    let mut applied = super::snapshot::read(&compose.root);
    let bin = super::team_mcp_bin().display().to_string();
    let mut next = super::snapshot::compute(&compose, &bin);
    if scoped.is_some() {
        next.compose_digest = applied
            .as_ref()
            .map(|s| s.compose_digest.clone())
            .unwrap_or_default();
    }
    // Managed scripts are root infrastructure and must exist on a first
    // project-scoped launch. Registration/rendering remain project-scoped.
    ensure_wrapper_and_dirs(&compose)?;
    // Register and render only the selected project's roster, leaving
    // sibling project state untouched.
    if scoped.is_none() {
        render_all_public(&compose)?;
        register_all_public(&compose)?;
        ensure_claude_trust(&compose)?;
    } else {
        // Per-project work: re-render the named project's env+mcp
        // (operator may have edited them) and pre-accept Claude trust
        // for that project's cwds. Both are idempotent and project-
        // scoped on disk.
        render_project_public(&compose, scoped.as_deref().unwrap())?;
        register_project_public(&compose, scoped.as_deref().unwrap())?;
        ensure_claude_trust_for_project(&compose, scoped.as_deref().unwrap())?;
    }

    reserve_in_registry(&compose, scoped.as_deref())?;
    let sup = TmuxSupervisor;
    let previous = applied.clone();
    let mut touched = start_agents_with(
        &compose,
        &sup,
        previous.as_ref(),
        scoped.as_deref(),
        targets.as_ref(),
        fresh,
        |id| {
            let snap =
                super::snapshot::merge_applied(applied.as_ref(), &next, [id.to_string()], []);
            super::snapshot::write(&compose.root, &snap)?;
            applied = Some(snap);
            Ok(())
        },
    )?;

    // Spawn one team-bot per manager that carries a `telegram:` block.
    // Each bot runs in its own tmux session and is scoped via
    // --manager so DMs reach exactly that manager.
    let team_bot = super::bot::team_bot_bin();
    for spec in super::bot::bot_specs(&compose) {
        let split = spec.manager.split_once(':');
        if scoped
            .as_deref()
            .is_some_and(|id| split.map(|(p, _)| p) != Some(id))
        {
            continue;
        }
        if targets
            .as_ref()
            .is_some_and(|t| !t.contains(split.map(|(_, a)| a).unwrap_or("")))
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
        println!("no agents in scope for project {id}.");
    }
    record_in_registry(&compose, scoped.as_deref());
    if touched > 0 {
        super::caffeinate::ensure_running();
    }
    Ok(())
}

/// Start only stopped agents, checkpointing each successful spawn. A
/// skipped process still runs its old inputs, so it never advances applied
/// state. Injected supervision keeps failure/recovery tests off real tmux.
fn start_agents_with(
    compose: &Compose,
    sup: &impl Supervisor,
    prior: Option<&super::snapshot::Snapshot>,
    scoped: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    fresh: bool,
    mut checkpoint: impl FnMut(&str) -> Result<()>,
) -> Result<usize> {
    let mut touched = 0;
    for h in compose.agents() {
        if scoped.is_some_and(|id| id != h.project) {
            continue;
        }
        if targets.is_some_and(|t| !t.contains(h.agent)) {
            continue;
        }
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        let mut observed = spec.clone();
        if let Some(entry) = prior.and_then(|snapshot| snapshot.agents.get(&h.id())) {
            observed.tmux_session = entry.tmux_session.clone();
        }
        let running = sup.state(&observed)? == AgentState::Running
            || (observed.tmux_session != spec.tmux_session
                && sup.state(&spec)? == AgentState::Running);
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
            freshen_for_spec(&compose.root, &spec, &h.spec.runtime, fresh);
        }
        if !running {
            sup.up(&spec)?;
            checkpoint(&h.id())?;
        }
        println!("up · {}{}", h.id(), fresh_suffix(fresh && !running));
        touched += 1;
    }

    Ok(touched)
}

mod artifacts;
mod freshen;
mod registration;
mod registry;
mod trust;

pub use artifacts::{ensure_wrapper_and_dirs, render_all_public, render_project_public};
#[cfg(test)]
use artifacts::{DEFAULT_BOOT_SCRIPT, DEFAULT_WRAPPER};
pub(crate) use freshen::{fresh_suffix, freshen_for_spec};
#[cfg(test)]
use freshen::{freshen_action, FreshenAction};
pub use registration::{register_all_public, register_project_public};
pub(super) use registry::{guard_no_name_collision, record_in_registry, reserve_in_registry};
#[cfg(test)]
use registry::{guard_no_name_collision_in, record_in_registry_in, registry_entries};
use trust::{ensure_claude_trust, ensure_claude_trust_for_project};

/// Pull `<root>/.env` (and `<root>/../.env`) into the process so the
/// tmux session for `team-bot` inherits the bot token + chat-ids the
/// operator wrote with `teamctl bot setup`. Mirrors the loader in
/// `cmd::env::run`. Idempotent — never overwrites a value already in
/// the environment.
pub(crate) fn source_dotenv_into_process(root: &std::path::Path) {
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
mod tests;

#[cfg(test)]
pub(crate) mod regression_tests;
