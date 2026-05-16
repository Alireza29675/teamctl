use std::path::Path;

use anyhow::Result;
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

use super::agent_filter::AgentSelector;

pub fn run(root: &Path, project: Option<&str>, sel: &AgentSelector) -> Result<()> {
    let compose = super::load(root)?;
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
    Ok(())
}
