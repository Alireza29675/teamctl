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

pub fn run(root: &Path, dry_run: bool, project: Option<&str>) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("error: {e}");
        }
        anyhow::bail!("{} validation error(s) — fix before reload", errs.len());
    }
    let scoped = project
        .map(|name| super::project_filter::resolve(&compose, name))
        .transpose()?;

    let prev = snapshot::read(&compose.root);
    let bin = super::team_mcp_bin().display().to_string();
    let next = snapshot::compute(&compose, &bin);

    // Fast path: compose file unchanged AND no rendered diff. The
    // compose_digest covers the on-disk YAML; the per-agent
    // fingerprints cover everything that flows from compose +
    // role_prompt files. Together they're a tight "nothing applied,
    // nothing to do" check.
    let mut plan = snapshot::plan(prev.as_ref(), &next);
    if let Some(id) = scoped.as_deref() {
        plan = filter_plan_to_project(plan, id);
    }
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

    // Per T-133: scoped runs skip the global wrapper/DB/snapshot
    // rewrites because each clobbers state owned by other projects.
    // Per-project artefact rendering still happens so a freshly-
    // edited env or mcp file lands before the supervisor restarts.
    if let Some(id) = scoped.as_deref() {
        super::up::render_project_public(&compose, id)?;
    } else {
        super::up::ensure_wrapper_and_dirs(&compose)?;
        super::up::render_all_public(&compose)?;
        super::up::register_all_public(&compose)?;
    }

    apply_plan(&compose, &plan)?;
    if scoped.is_none() {
        snapshot::write(&compose.root, &next)?;
    }
    Ok(())
}

/// Filter a plan down to entries whose agent id begins with
/// `<project_id>:`. Used when `teamctl reload` is invoked with a
/// project arg — the diff is computed across the whole compose, but
/// only the named project's portion gets applied. The kept ids are
/// untouched in the plan; the next unscoped reload will diff against
/// the original snapshot and reconcile any project the scoped run
/// missed.
fn filter_plan_to_project(plan: ReloadPlan, project_id: &str) -> ReloadPlan {
    let prefix = format!("{project_id}:");
    let in_project = |id: &str| id.starts_with(&prefix);
    ReloadPlan {
        add: plan.add.into_iter().filter(|id| in_project(id)).collect(),
        change: plan
            .change
            .into_iter()
            .filter(|(id, _)| in_project(id))
            .collect(),
        remove: plan
            .remove
            .into_iter()
            .filter(|r| in_project(&r.id))
            .collect(),
        keep: plan.keep.into_iter().filter(|id| in_project(id)).collect(),
        change_prior: plan
            .change_prior
            .into_iter()
            .filter(|(id, _)| in_project(id))
            .collect(),
    }
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
    use crate::cmd::snapshot::{
        AgentEntry, ChangedInputs, Fingerprints, PromptFingerprint, RemovedAgent,
    };
    use std::collections::BTreeMap;

    #[test]
    fn drain_suffix_empty_on_graceful() {
        assert_eq!(drain_suffix(DrainOutcome::Graceful), "");
    }

    #[test]
    fn drain_suffix_annotates_timeout() {
        assert!(drain_suffix(DrainOutcome::TimedOutKilled).contains("drain timed out"));
    }

    fn entry(env: &str) -> AgentEntry {
        AgentEntry {
            tmux_session: "a-x".into(),
            env_file: env.into(),
            fingerprints: Fingerprints {
                env: String::new(),
                mcp: String::new(),
                role_prompt: PromptFingerprint::None,
            },
        }
    }

    fn removed(id: &str) -> RemovedAgent {
        RemovedAgent {
            id: id.into(),
            tmux_session: format!("a-{id}"),
            env_file: PathBuf::from(""),
        }
    }

    fn changed_inputs() -> ChangedInputs {
        ChangedInputs {
            env: true,
            mcp: false,
            role_prompt: false,
        }
    }

    #[test]
    fn filter_plan_keeps_only_matching_project_entries() {
        // Whole-tree plan covers two projects; scoped reload trims it
        // to one. The other project's add/change/remove/keep entries
        // disappear so apply_plan never touches them.
        let mut change_prior = BTreeMap::new();
        change_prior.insert("a:m".into(), entry("/tmp/a-m.env"));
        change_prior.insert("b:m".into(), entry("/tmp/b-m.env"));
        let plan = ReloadPlan {
            add: vec!["a:w".into(), "b:w".into()],
            change: vec![
                ("a:m".into(), changed_inputs()),
                ("b:m".into(), changed_inputs()),
            ],
            remove: vec![removed("a:gone"), removed("b:gone")],
            keep: vec!["a:keep".into(), "b:keep".into()],
            change_prior,
        };

        let filtered = filter_plan_to_project(plan, "a");
        assert_eq!(filtered.add, vec!["a:w"]);
        assert_eq!(filtered.change.len(), 1);
        assert_eq!(filtered.change[0].0, "a:m");
        assert_eq!(filtered.remove.len(), 1);
        assert_eq!(filtered.remove[0].id, "a:gone");
        assert_eq!(filtered.keep, vec!["a:keep"]);
        assert_eq!(filtered.change_prior.len(), 1);
        assert!(filtered.change_prior.contains_key("a:m"));
    }

    #[test]
    fn filter_plan_does_not_match_prefix_collisions() {
        // Project ids `a` and `aa` share a prefix but the filter
        // separates them — `aa:m` does not start with `a:` and stays
        // out of the project-`a` slice.
        let plan = ReloadPlan {
            add: vec!["a:m".into(), "aa:m".into(), "ab:m".into()],
            ..ReloadPlan::default()
        };
        let filtered = filter_plan_to_project(plan, "a");
        assert_eq!(filtered.add, vec!["a:m"]);
    }

    #[test]
    fn filter_plan_returns_empty_when_no_entries_match() {
        let plan = ReloadPlan {
            add: vec!["a:m".into(), "b:m".into()],
            ..ReloadPlan::default()
        };
        let filtered = filter_plan_to_project(plan, "z");
        assert!(filtered.is_empty());
    }
}
