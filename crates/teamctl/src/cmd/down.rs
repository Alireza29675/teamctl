use std::path::Path;

use anyhow::{bail, Result};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

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
