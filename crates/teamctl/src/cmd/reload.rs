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
//!    drain `remove` and the prior side of `change` using the
//!    persisted spec (SIGINT → poll → kill-session via
//!    `Supervisor::drain`), then bring up `add` and `change` with the
//!    freshly computed spec.
//! 6. Persist the next snapshot.
//!
//! `--dry-run` exits after step 3 with the plan printed but no files
//! rendered, no agents touched, no snapshot written. The plan output
//! is identical to the apply output (with a `(dry run)` annotation),
//! so preview and apply cannot drift.
//!
//! Hashing is `blake3` throughout (see `snapshot::hash_*`).
//! File locking on `applied.json` and an audit log land in PR C/D —
//! the schema is forward-compatible with each.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, DrainOutcome, Supervisor, TmuxSupervisor};

use super::snapshot::{self, AgentEntry, ReloadPlan, RemovedAgent};

pub fn run(root: &Path, dry_run: bool) -> Result<()> {
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
    let no_changes = plan.is_empty()
        && prev
            .as_ref()
            .map(|s| s.compose_digest == next.compose_digest && s.global == next.global)
            .unwrap_or(false);
    if no_changes {
        if dry_run {
            println!("no changes (dry run)");
        } else {
            println!("no changes");
        }
        return Ok(());
    }

    if dry_run {
        print_plan(&plan, true);
        return Ok(());
    }

    super::up::ensure_wrapper_and_dirs(&compose)?;
    super::up::render_all_public(&compose)?;
    super::up::register_all_public(&compose)?;

    apply_plan(&compose, &plan)?;
    snapshot::write(&compose.root, &next)?;
    Ok(())
}

/// Write the plan to stdout in the same per-line format the apply
/// path produces, with a `(dry run)` annotation. Used by `--dry-run`
/// so the operator sees exactly the lines a real reload would print.
fn print_plan(plan: &ReloadPlan, dry: bool) {
    let suffix = if dry { " (dry run)" } else { "" };
    for r in &plan.remove {
        println!("removed · {}{suffix}", r.id);
    }
    for (id, inputs) in &plan.change {
        println!("changed · {id} ({}){suffix}", inputs.label());
    }
    for id in &plan.add {
        println!("added   · {id}{suffix}");
    }
}

fn apply_plan(compose: &Compose, plan: &ReloadPlan) -> Result<()> {
    let sup = TmuxSupervisor;
    let drain_timeout = Duration::from_secs(compose.global.supervisor.drain_timeout_secs);

    // Removals: drain using the *prior* tmux_session — the one that
    // was actually started for this agent. Reconstructing from the
    // current compose's tmux_prefix would silently leak the session
    // when the prefix changed. Drain (rather than down) gives the
    // agent a chance to flush in-flight work.
    for r in &plan.remove {
        let outcome = sup.drain(&spec_from_removed(compose, r), drain_timeout)?;
        println!("removed · {}{}", r.id, drain_suffix(outcome));
    }

    // Changes: drain the prior spec, then start fresh with the
    // current spec.
    for (id, inputs) in &plan.change {
        let prior = plan
            .change_prior
            .get(id)
            .expect("change_prior populated by plan()");
        let outcome = sup.drain(&spec_from_prior(compose, id, prior), drain_timeout)?;
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            sup.up(&spec)?;
        }
        println!(
            "changed · {id} ({}){}",
            inputs.label(),
            drain_suffix(outcome)
        );
    }

    // Additions: fresh spec, fresh up.
    for id in &plan.add {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            sup.up(&spec)?;
            println!("added   · {id}");
        }
    }

    // Kept agents that somehow stopped (e.g. tmux session crashed)
    // get restarted in place. Same behaviour as v1 reload.
    for id in &plan.keep {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            if sup.state(&spec)? == AgentState::Stopped {
                sup.up(&spec)?;
                println!("started · {id}");
            }
        }
    }
    Ok(())
}

/// One-word annotation surfaced in the per-line restart log when
/// drain fell through to a hard kill. Operator signal that
/// `drain_timeout_secs` may need tuning.
fn drain_suffix(outcome: DrainOutcome) -> &'static str {
    match outcome {
        DrainOutcome::Graceful => "",
        DrainOutcome::TimedOutKilled => " [drain timed out — killed]",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_suffix_empty_on_graceful() {
        assert_eq!(drain_suffix(DrainOutcome::Graceful), "");
    }

    #[test]
    fn drain_suffix_annotates_timeout() {
        assert!(drain_suffix(DrainOutcome::TimedOutKilled).contains("drain timed out"));
    }
}
