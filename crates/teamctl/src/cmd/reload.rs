//! Diff-based reload.
//!
//! Hash each agent's rendered artifact set; if the hash differs from the
//! last-applied snapshot, restart that agent. Unchanged agents are left
//! alone. The snapshot lives at `state/applied.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use team_core::compose::Compose;
use team_core::render::render_agent;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

#[derive(Default, Serialize, Deserialize)]
struct Applied {
    agents: BTreeMap<String, String>, // id -> hash
}

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        anyhow::bail!("{} validation error(s) — fix before reload", errs.len());
    }

    let applied_path = compose.root.join("state/applied.json");
    let previous: Applied = fs::read_to_string(&applied_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let current = hash_agents(&compose);
    let diff = diff_applied(&previous, &current);

    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("no changes");
        return Ok(());
    }

    // Re-render (cheap, idempotent) and (re)register agents on add/change.
    super::up::ensure_wrapper_and_dirs(&compose)?;
    super::up::render_all_public(&compose)?;
    super::up::register_all_public(&compose)?;

    let sup = TmuxSupervisor;
    for id in &diff.removed {
        let (project, agent) = id.split_once(':').unwrap_or((id.as_str(), ""));
        let spec = AgentSpec {
            project: project.into(),
            agent: agent.into(),
            tmux_session: format!("{}{project}-{agent}", compose.global.supervisor.tmux_prefix),
            wrapper: compose.root.join("bin/agent-wrapper.sh"),
            cwd: compose.root.clone(),
            env_file: compose
                .root
                .join(format!("state/envs/{project}-{agent}.env")),
        };
        sup.down(&spec)?;
        println!("removed · {id}");
    }
    for h in compose.agents() {
        let id = h.id();
        let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
        if diff.added.contains(&id) {
            sup.up(&spec)?;
            println!("added   · {id}");
        } else if diff.changed.contains(&id) {
            sup.down(&spec)?;
            sup.up(&spec)?;
            println!("changed · {id}");
        } else if sup.state(&spec)? == AgentState::Stopped {
            sup.up(&spec)?;
            println!("started · {id}");
        }
    }

    fs::create_dir_all(applied_path.parent().unwrap())?;
    fs::write(&applied_path, serde_json::to_string_pretty(&current)?)
        .context("write applied snapshot")?;
    Ok(())
}

struct Diff {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
}

fn diff_applied(prev: &Applied, next: &Applied) -> Diff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (k, v) in &next.agents {
        match prev.agents.get(k) {
            None => added.push(k.clone()),
            Some(p) if p != v => changed.push(k.clone()),
            _ => {}
        }
    }
    for k in prev.agents.keys() {
        if !next.agents.contains_key(k) {
            removed.push(k.clone());
        }
    }
    Diff {
        added,
        removed,
        changed,
    }
}

fn hash_agents(compose: &Compose) -> Applied {
    let bin = super::team_mcp_bin().display().to_string();
    let mut agents = BTreeMap::new();
    for h in compose.agents() {
        let (env, mcp) = render_agent(compose, h, &bin);
        let prompt_bytes = h
            .spec
            .role_prompt
            .as_ref()
            .and_then(|p| fs::read(compose.root.join(p)).ok())
            .unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        env.hash(&mut hasher);
        mcp.hash(&mut hasher);
        prompt_bytes.hash(&mut hasher);
        agents.insert(h.id(), format!("{:016x}", hasher.finish()));
    }
    Applied { agents }
}
