use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

use super::agent_filter::AgentSelector;

pub fn run(root: &Path, project: Option<&str>, sel: &AgentSelector) -> Result<()> {
    let compose = super::load(root)?;
    // T-310: gate on validation before the supervisor builds any
    // shell-bound command. `up` and `reload` already do this; `down`
    // didn't, leaving `build_up_command`'s `{project}:{agent}`
    // interpolation reachable from a shell-metacharacter id via the
    // `down` path. Mirror the existing up/reload shape so a malicious
    // compose can't slip past on this command either.
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        bail!("{} validation error(s) — fix before down", errs.len());
    }
    let scoped = project
        .map(|name| super::project_filter::resolve(&compose, name))
        .transpose()?;
    // Per-agent target set (T-305). `None` => no agent-level filter
    // (the no-arg / `<project>`-only contracts, untouched). The
    // selector is only ever scoped alongside a project (clap enforces
    // `requires = "project"`), so `resolve` is only reached when
    // `scoped` is `Some`.
    let targets = match scoped.as_deref() {
        Some(id) => super::agent_filter::resolve(&compose, id, sel)?,
        None => None,
    };
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
        sup.down(&spec)?;
        println!("down · {}", h.id());
        touched += 1;
    }
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
        // scope, skip the bot unless its manager is in the target set.
        // `targets` is `Some` only when `scoped` is `Some`, so the
        // guard above has already pinned `split` to the in-scope
        // project's pair here.
        if let Some(t) = &targets {
            if !t.contains(split.map(|(_, a)| a).unwrap_or("")) {
                continue;
            }
        }
        super::bot::down_one(&spec);
        println!("down · bot {}", spec.session);
        touched += 1;
    }
    // T-468: reap orphaned sessions — agents recorded as up at this root by
    // a prior `up` but no longer in the compose. The loops above only tear
    // down current-YAML agents, so a removed agent's session would otherwise
    // linger forever. Read the registry BEFORE the clear below; honors the
    // same scope as the teardown (whole-team / one project / skip on a
    // per-agent `--agent` down).
    touched += reap_orphans(&compose, &sup, scoped.as_deref(), targets.is_some());

    if let (Some(id), 0) = (scoped.as_deref(), touched) {
        println!("no agents in scope for project {id}.");
    }

    // T-466: clear this team's rows from the durable system-wide registry
    // now it's down, so `teamctl ps` and orphan reaping don't report a dead
    // team. A per-agent down only partially tears the team down, so it stays
    // registered (`registry_clear_scope` returns `None`). Best-effort: never
    // fail teardown on a registry error.
    if let Some(project) = registry_clear_scope(targets.is_some(), scoped.as_deref()) {
        if let Some(dir) = team_core::registry::config_dir() {
            if let Err(e) = team_core::registry::clear(&dir, &compose.root, project) {
                eprintln!("warn · teams registry: {e:#}");
            }
        }
    }

    // T-370: release the host-level keep-awake once the last teamctl team on
    // this host is down (host-wide refcount via the `@teamctl` tmux tagging).
    // macOS-only, no-op elsewhere; a stale/dead pid is reaped without error.
    super::caffeinate::stop_if_last();
    Ok(())
}

/// What `down` should clear from the registry.
///
/// - `per_agent` (a `--agent` selector is active) ⇒ `None`: only some of
///   the team's agents went down, so the team stays registered.
/// - otherwise ⇒ `Some(scoped)`: clear that scope — `None` clears every
///   entry for this root (whole-team `down`), `Some(id)` clears just the
///   one project (so siblings sharing the root stay registered).
///
/// Pure so the decision is unit-testable without a registry or `$HOME`.
fn registry_clear_scope(per_agent: bool, scoped: Option<&str>) -> Option<Option<&str>> {
    if per_agent {
        None
    } else {
        Some(scoped)
    }
}

/// Drain leftover tmux sessions for agents recorded as up at this root (in
/// the durable registry) but no longer in the compose — the orphans a plain
/// `down` misses. Only sessions that are still *running* are killed and
/// announced: a roster row whose session already died (crashed, or reaped by
/// an earlier `down`) is skipped, so `down` never prints a phantom `reaped`.
/// Returns the count actually reaped. Best-effort — a missing/unreadable
/// registry or a supervisor error never fails an otherwise-successful
/// `down`. Shares `reap_targets`' scope contract (skips on a per-agent down).
fn reap_orphans(
    compose: &Compose,
    sup: &TmuxSupervisor,
    scoped: Option<&str>,
    per_agent: bool,
) -> usize {
    let Some(dir) = team_core::registry::config_dir() else {
        return 0;
    };
    let desired: HashSet<String> = compose.agents().map(|h| h.id()).collect();
    let orphans = match team_core::registry::orphans_for_root(
        &dir,
        &compose.root,
        &desired,
        scoped,
        per_agent,
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("warn · teams registry: {e:#}");
            return 0;
        }
    };
    let mut reaped = 0;
    for orphan in orphans {
        let spec = orphan_spec(compose, &orphan);
        match sup.state(&spec) {
            // Only a session that's actually up is something to reap.
            Ok(AgentState::Running) => match sup.down(&spec) {
                Ok(()) => {
                    println!("reaped · {}", orphan.id());
                    reaped += 1;
                }
                Err(e) => eprintln!("warn · reap {}: {e:#}", orphan.id()),
            },
            // Already gone — the registry row is stale, nothing to kill.
            Ok(_) => {}
            Err(e) => eprintln!("warn · reap {} (state): {e:#}", orphan.id()),
        }
    }
    reaped
}

/// Build a teardown spec for an orphan from its registry roster entry. The
/// agent's compose handle is gone, so the spec is assembled directly: only
/// `tmux_session` matters to `TmuxSupervisor::down` (it runs
/// `tmux kill-session -t <session>`); the other fields are inert here.
fn orphan_spec(compose: &Compose, e: &team_core::registry::RosterEntry) -> AgentSpec {
    AgentSpec {
        project: e.project_id.clone(),
        agent: e.agent.clone(),
        tmux_session: e.tmux_session.clone(),
        wrapper: super::agent_wrapper(&compose.root),
        cwd: compose.root.clone(),
        env_file: PathBuf::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_agent_down_leaves_registry_untouched() {
        // A `--agent` selector only partially tears the team down.
        assert_eq!(registry_clear_scope(true, None), None);
        assert_eq!(registry_clear_scope(true, Some("main")), None);
    }

    #[test]
    fn whole_team_down_clears_every_entry_for_the_root() {
        assert_eq!(registry_clear_scope(false, None), Some(None));
    }

    #[test]
    fn project_scoped_down_clears_only_that_project() {
        assert_eq!(
            registry_clear_scope(false, Some("main")),
            Some(Some("main"))
        );
    }
}
