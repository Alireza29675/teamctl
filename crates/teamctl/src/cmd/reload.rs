//! Diff-based reload, driven by `state/applied.json` schema v2.
//!
//! The reload algorithm:
//!
//! 1. Load the prior snapshot (`snapshot::read`). A missing, corrupt,
//!    or schema-v1 file is treated as "no prior" — every current agent
//!    becomes `add` and the next reload re-establishes the spine.
//! 2. Compute the next snapshot from the live compose
//!    (`snapshot::compute`). Per-agent fingerprints split into env,
//!    mcp, and `role_prompt` (with `None`/`Missing`/`Present`
//!    sentinels).
//! 3. Build a `ReloadPlan` (`snapshot::plan`) with `add`, `change`,
//!    `remove`, `keep`. The plan carries the *prior* `AgentEntry` for
//!    `change` and `remove` so teardown targets the actually-running
//!    tmux session — correct even when `tmux_prefix` has drifted since
//!    the last apply.
//! 4. Fast-path: if `compose_digest` matches and the plan is empty,
//!    print "no changes" and return.
//! 5. Apply: render artefacts, register changed/added in the mailbox,
//!    tear down `remove` and the prior side of `change` using the
//!    persisted spec, then bring up `add` and `change` with the freshly
//!    computed spec.
//! 6. Persist the next snapshot.
//!
//! Hashing is `blake3` throughout (see `snapshot::hash_*`).
//! Graceful drain (SIGTERM → wait → kill), a `--dry-run` flag, file
//! locking on `applied.json`, and an audit log all land in PR B/C/D —
//! the schema is forward-compatible with each.

use std::path::{Path, PathBuf};

use anyhow::Result;
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

use super::snapshot::{self, AgentEntry, ReloadPlan, RemovedAgent};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        anyhow::bail!("{} validation error(s) — fix before reload", errs.len());
    }

    let prev = snapshot::read(&compose.root);
    let bin = super::team_mcp_bin().display().to_string();
    let next = snapshot::compute(&compose, &bin);

    // Fast path: compose file unchanged AND no rendered diff. The
    // compose_digest covers the on-disk YAML; the per-agent
    // fingerprints cover everything that flows from compose +
    // role_prompt files. Together they're a tight "nothing applied,
    // nothing to do" check.
    let plan = snapshot::plan(prev.as_ref(), &next);
    if plan.is_empty()
        && prev
            .as_ref()
            .map(|s| s.compose_digest == next.compose_digest && s.global == next.global)
            .unwrap_or(false)
    {
        println!("no changes");
        return Ok(());
    }

    super::up::ensure_wrapper_and_dirs(&compose)?;
    super::up::render_all_public(&compose)?;
    super::up::register_all_public(&compose)?;

    apply_plan(&compose, &plan)?;
    snapshot::write(&compose.root, &next)?;
    Ok(())
}

fn apply_plan(compose: &Compose, plan: &ReloadPlan) -> Result<()> {
    let sup = TmuxSupervisor;

    // Removals: tear down using the *prior* tmux_session — the one
    // that was actually started for this agent. Reconstructing from
    // the current compose's tmux_prefix would silently leak the
    // session when the prefix changed.
    for r in &plan.remove {
        sup.down(&spec_from_removed(compose, r))?;
        println!("removed · {}", r.id);
    }

    // Changes: drain (PR B will replace down() with drain()) using
    // the prior spec, then start fresh with the current spec.
    for (id, inputs) in &plan.change {
        let prior = plan
            .change_prior
            .get(id)
            .expect("change_prior populated by plan()");
        sup.down(&spec_from_prior(compose, id, prior))?;
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            sup.up(&spec)?;
        }
        println!("changed · {id} ({})", inputs.label());
    }

    // Additions: fresh spec, fresh up.
    for id in &plan.add {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            sup.up(&spec)?;
            println!("added   · {id}");
        }
    }

    // Kept agents that somehow stopped (e.g. tmux session crashed)
    // get restarted in place. Same behaviour as v1 reload.
    for id in &plan.keep {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec = AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            if sup.state(&spec)? == AgentState::Stopped {
                sup.up(&spec)?;
                println!("started · {id}");
            }
        }
    }
    Ok(())
}

fn spec_from_removed(compose: &Compose, r: &RemovedAgent) -> AgentSpec {
    let (project, agent) = r.id.split_once(':').unwrap_or((r.id.as_str(), ""));
    AgentSpec {
        project: project.into(),
        agent: agent.into(),
        tmux_session: r.tmux_session.clone(),
        wrapper: super::agent_wrapper(&compose.root),
        cwd: compose.root.clone(),
        env_file: r.env_file.clone(),
    }
}

fn spec_from_prior(compose: &Compose, id: &str, prior: &AgentEntry) -> AgentSpec {
    let (project, agent) = id.split_once(':').unwrap_or((id, ""));
    AgentSpec {
        project: project.into(),
        agent: agent.into(),
        tmux_session: prior.tmux_session.clone(),
        wrapper: super::agent_wrapper(&compose.root),
        cwd: compose.root.clone(),
        env_file: PathBuf::from(&prior.env_file),
    }
}
