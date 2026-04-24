use std::path::Path;

use anyhow::Result;
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let sup = TmuxSupervisor;
    for h in compose.agents() {
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        sup.down(&spec)?;
        println!("down · {}", h.id());
    }
    Ok(())
}
