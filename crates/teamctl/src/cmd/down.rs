use std::path::Path;

use anyhow::Result;
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, project: Option<&str>) -> Result<()> {
    let compose = super::load(root)?;
    let scoped = project
        .map(|name| super::project_filter::resolve(&compose, name))
        .transpose()?;
    let mut touched = 0usize;
    let sup = TmuxSupervisor;
    for h in compose.agents() {
        if scoped.as_deref().is_some_and(|id| id != h.project) {
            continue;
        }
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        sup.down(&spec)?;
        println!("down · {}", h.id());
        touched += 1;
    }
    for spec in super::bot::bot_specs(&compose) {
        if scoped
            .as_deref()
            .is_some_and(|id| spec.manager.split_once(':').map(|(p, _)| p) != Some(id))
        {
            continue;
        }
        super::bot::down_one(&spec);
        println!("down · bot {}", spec.session);
        touched += 1;
    }
    if let (Some(id), 0) = (scoped.as_deref(), touched) {
        println!("no agents in project {id}.");
    }
    Ok(())
}
