use std::path::Path;

use anyhow::Result;
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, clean_worktrees: bool) -> Result<()> {
    let compose = super::load(root)?;
    let sup = TmuxSupervisor;
    for h in compose.agents() {
        let spec = AgentSpec::from_handle(h, &compose);
        sup.down(&spec)?;
        println!("down · {}", h.id());
    }
    for spec in super::bot::bot_specs(&compose) {
        super::bot::down_one(&spec);
        println!("down · bot {}", spec.session);
    }
    if clean_worktrees {
        clean_agent_worktrees(&compose);
    }
    Ok(())
}

/// Destructive: drop every per-agent worktree directory + its
/// `agents/<agent-id>` branch. Default `teamctl down` preserves them
/// (matches the existing "state preserved across `down && up`"
/// promise); operators opt in via `--clean-worktrees`.
fn clean_agent_worktrees(compose: &team_core::compose::Compose) {
    if !compose.global.supervisor.worktree_isolation_enabled() {
        return;
    }
    for h in compose.agents() {
        if h.spec.cwd_override.is_some() {
            continue;
        }
        let Some(git_source) = compose.resolve_project_cwd(h.project) else {
            continue;
        };
        let wt = team_core::worktree::default_worktree_path(&compose.root, h.agent);
        if let Err(e) = team_core::worktree::remove_worktree(&git_source, &wt, h.agent) {
            eprintln!("warn · clean worktree for {}: {e:#}", h.id());
        } else {
            println!("clean · worktree {}", h.id());
        }
    }
}
