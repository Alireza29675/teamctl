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

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, DrainOutcome, Supervisor, TmuxSupervisor};

use super::agent_filter::AgentSelector;
use super::snapshot::{self, AgentEntry, ReloadPlan, RemovedAgent};

pub fn run(
    root: &Path,
    dry_run: bool,
    project: Option<&str>,
    sel: &AgentSelector,
    fresh: bool,
    force: bool,
) -> Result<()> {
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

    // T-305: a per-agent scope force-restarts exactly the selected
    // agents regardless of whether their config changed. This is a
    // distinct path from the diff-driven reload — the unscoped and
    // `<project>`-only contracts below are left untouched. clap
    // guarantees the selector only appears with a project, so
    // `scoped` is `Some` here.
    if sel.is_scoped() {
        if let Some(id) = scoped.as_deref() {
            let targets = super::agent_filter::resolve(&compose, id, sel)?
                .expect("scoped selector resolves to a concrete agent set");
            return force_restart_scoped(&compose, id, &targets, dry_run, fresh);
        }
    }

    let prev = snapshot::read(&compose.root);
    let bin = super::team_mcp_bin().display().to_string();
    let next = snapshot::compute(&compose, &bin);

    // Fast path: compose file unchanged AND no rendered diff. The
    // compose_digest covers the on-disk YAML; the per-agent
    // fingerprints cover everything that flows from compose +
    // role_prompt files. Together they're a tight "nothing applied,
    // nothing to do" check.
    let mut plan = snapshot::plan(prev.as_ref(), &next);
    // T-468: when applied.json is absent (`prev` is `None`), `snapshot::plan`
    // yields an EMPTY remove-set, so a reload can't reap an agent deleted from
    // the YAML. Fall back to the durable registry: any agent it records as up
    // at this root but no longer in the compose is an orphan — inject those as
    // removals so `apply_plan` drains them. `orphan_removals` gates this to the
    // `prev`-absent case; when the snapshot diff is available it already
    // computes removals, so injecting would double-reap.
    plan.remove
        .extend(orphan_removals(prev.is_some(), registry_orphans(&compose)));
    if let Some(id) = scoped.as_deref() {
        plan = filter_plan_to_project(plan, id);
    }
    // T-384: `--force` restarts every agent in scope regardless of the
    // diff. Drag the kept (unchanged, still-running) agents into the
    // change set so they are drained and brought back up like a changed
    // agent. Added / removed / genuinely-changed agents are unaffected.
    // Runs after project filtering so a `<project>`-scoped force only
    // bounces that project's agents.
    if force {
        force_promote_keeps(&mut plan, prev.as_ref());
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
        print_plan(&plan, true, fresh);
        return Ok(());
    }

    // Per T-133: scoped runs skip the global wrapper rewrite and the
    // whole-tree DB rewrite (they would clobber other projects'
    // state). Per-project artefact rendering still happens so
    // freshly-edited env or mcp files land before the supervisor
    // restarts. The snapshot is still written below — but merged
    // into the prior snapshot rather than replacing it.
    if let Some(id) = scoped.as_deref() {
        super::up::render_project_public(&compose, id)?;
    } else {
        super::up::ensure_wrapper_and_dirs(&compose)?;
        super::up::render_all_public(&compose)?;
        super::up::register_all_public(&compose)?;
    }

    apply_plan(&compose, &plan, fresh)?;
    // Persist the snapshot. Scoped runs merge the named project's
    // per-agent entries into the existing applied.json (T-133) —
    // preserves diff correctness for the next unscoped reload without
    // clobbering other projects' last-applied fingerprints.
    let snap = match scoped.as_deref() {
        Some(id) => snapshot::merge_project_into(prev.as_ref(), &next, id),
        None => next,
    };
    snapshot::write(&compose.root, &snap)?;
    // T-468: refresh the durable registry to the post-reload roster so
    // `teamctl ps` and orphan reaping reflect what's now up. This is the
    // #466 registry-write deferred from up-only into the reload path; the
    // upsert preserves each team's original `started_at`, so uptime doesn't
    // reset on a reload. (A project removed *entirely* from the YAML leaves
    // its row until a whole-team `down` clears the root — benign: `ps` reads
    // live tmux, not this store, and a later reap re-kills its already-dead
    // sessions harmlessly.) Best-effort.
    super::up::record_in_registry(&compose, scoped.as_deref());
    Ok(())
}

/// T-305: force-restart exactly the selected agents, regardless of
/// whether their config changed. Distinct from the diff-driven path —
/// `reload <project> <agent>…` / `--except` always bounces the scoped
/// set. Unscoped and `<project>`-only reload never reach here.
///
/// Mirrors the diff path's restart shape: drain the *actually-running*
/// session (preferring the prior snapshot entry so a drifted
/// `tmux_prefix` still tears down the real session), then bring the
/// agent back up with its current spec.
fn force_restart_scoped(
    compose: &Compose,
    project_id: &str,
    targets: &BTreeSet<String>,
    dry_run: bool,
    fresh: bool,
) -> Result<()> {
    // Stable manager-then-worker order, matching `compose.agents()`
    // ordering used everywhere else in the CLI.
    let ids: Vec<String> = compose
        .agents()
        .filter(|h| h.project == project_id && targets.contains(h.agent))
        .map(|h| h.id())
        .collect();

    if ids.is_empty() {
        // e.g. `--except` named every agent. Mirror up/down's
        // empty-scope line rather than silently doing nothing.
        println!("no agents in scope for project {project_id}.");
        return Ok(());
    }

    if dry_run {
        for id in &ids {
            println!(
                "reloaded · {id} (forced){} (dry run)",
                super::up::fresh_suffix(fresh)
            );
        }
        return Ok(());
    }

    // Re-render the project's artefacts so a freshly-edited
    // env/mcp/role_prompt lands before the restart — same as the
    // diff-driven scoped reload. Idempotent for unchanged agents; the
    // cross-project DB rewrite stays skipped (T-133).
    super::up::render_project_public(compose, project_id)?;

    let prev = snapshot::read(&compose.root);
    let sup = TmuxSupervisor;
    let drain_timeout = Duration::from_secs(compose.global.supervisor.drain_timeout_secs);

    for id in &ids {
        // Drain the actually-running session. Prefer the prior
        // snapshot entry (same prefix-drift correctness argument as
        // the diff path's `spec_from_prior`); fall back to the current
        // spec when the agent was never applied. Draining a
        // not-running session returns immediately — no real wait.
        let drain_spec = match prev.as_ref().and_then(|s| s.agents.get(id)) {
            Some(e) => spec_from_prior(compose, id, e),
            None => match compose.agents().find(|h| &h.id() == id) {
                Some(h) => {
                    AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix)
                }
                None => continue,
            },
        };
        let outcome = sup.drain(&drain_spec, drain_timeout)?;

        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            super::up::freshen_for_spec(&spec, &h.spec.runtime, fresh);
            sup.up(&spec)?;
        }
        println!(
            "reloaded · {id} (forced){}{}",
            super::up::fresh_suffix(fresh),
            drain_suffix(outcome)
        );
    }

    // Persist the snapshot so the next *unscoped* reload diffs
    // correctly. Merge just this project's per-agent entries into the
    // prior snapshot (T-133) — other projects' fingerprints untouched.
    // If a forced agent's config also changed, the restart already
    // applied the new spec and the merged `next` records it.
    let bin = super::team_mcp_bin().display().to_string();
    let next = snapshot::compute(compose, &bin);
    let snap = snapshot::merge_project_into(prev.as_ref(), &next, project_id);
    snapshot::write(&compose.root, &snap)?;
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

/// T-384: `--force` support. Move every kept agent into the `change`
/// set so the apply path drains and restarts it even though its
/// fingerprints are unchanged. The prior snapshot entry (always present
/// for a keep, which by definition exists in both prev and next) is
/// copied into `change_prior` so teardown targets the actually-running
/// tmux session, prefix-drift-safe, exactly like the diff path. The
/// promoted entry carries `ChangedInputs::forced()` (all-false), which
/// the preview/apply output renders as `(forced)`.
fn force_promote_keeps(plan: &mut ReloadPlan, prev: Option<&snapshot::Snapshot>) {
    let keeps = std::mem::take(&mut plan.keep);
    for id in keeps {
        match prev.and_then(|s| s.agents.get(&id)) {
            Some(prior) => {
                plan.change_prior.insert(id.clone(), prior.clone());
                plan.change.push((id, snapshot::ChangedInputs::forced()));
            }
            // A keep without a prior entry can't happen (keeps come from
            // prev ∩ next); leave it kept so a stopped one is still
            // revived by apply_plan's keep loop.
            None => plan.keep.push(id),
        }
    }
}

/// The leading `verb · id (annotation)` for a `change` entry, shared by
/// preview and apply so the two can't drift. A genuine diff entry
/// reports which inputs changed (`changed · id (env+mcp)`); a
/// `--force`-promoted keep carries an all-false `ChangedInputs` and
/// reports `reloaded · id (forced)`, matching the scoped force path.
fn change_line_head(id: &str, inputs: &snapshot::ChangedInputs) -> String {
    if inputs.any() {
        format!("changed · {id} ({})", inputs.label())
    } else {
        format!("reloaded · {id} (forced)")
    }
}

/// Write the plan to stdout in the same per-line format the apply
/// path produces, with a `(dry run)` annotation. Used by `--dry-run`
/// so the operator sees exactly the lines a real reload would print.
fn print_plan(plan: &ReloadPlan, dry: bool, fresh: bool) {
    let dry_suffix = if dry { " (dry run)" } else { "" };
    let fresh_suffix = super::up::fresh_suffix(fresh);
    // Removals are torn down, never brought up — `(fresh)` doesn't apply.
    for r in &plan.remove {
        println!("removed · {}{dry_suffix}", r.id);
    }
    for (id, inputs) in &plan.change {
        println!("{}{fresh_suffix}{dry_suffix}", change_line_head(id, inputs));
    }
    for id in &plan.add {
        println!("added   · {id}{fresh_suffix}{dry_suffix}");
    }
}

fn apply_plan(compose: &Compose, plan: &ReloadPlan, fresh: bool) -> Result<()> {
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
            .expect("change_prior populated alongside every change entry");
        let outcome = sup.drain(&spec_from_prior(compose, id, prior), drain_timeout)?;
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            super::up::freshen_for_spec(&spec, &h.spec.runtime, fresh);
            sup.up(&spec)?;
        }
        println!(
            "{}{}{}",
            change_line_head(id, inputs),
            super::up::fresh_suffix(fresh),
            drain_suffix(outcome)
        );
    }

    // Additions: fresh spec, fresh up.
    for id in &plan.add {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            super::up::freshen_for_spec(&spec, &h.spec.runtime, fresh);
            sup.up(&spec)?;
            println!("added   · {id}{}", super::up::fresh_suffix(fresh));
        }
    }

    // Kept agents that somehow stopped (e.g. tmux session crashed)
    // get restarted in place. Same behaviour as v1 reload. `--fresh`
    // applies here too — a stopped agent we restart is genuinely
    // (re)spawned, so freshening it is consistent with every other
    // restart path.
    for id in &plan.keep {
        if let Some(h) = compose.agents().find(|h| &h.id() == id) {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            if sup.state(&spec)? == AgentState::Stopped {
                super::up::freshen_for_spec(&spec, &h.spec.runtime, fresh);
                sup.up(&spec)?;
                println!("started · {id}{}", super::up::fresh_suffix(fresh));
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

/// Gate the registry-orphan injection (T-468): the registry fallback fires
/// ONLY when the prior snapshot is absent (`prev_present == false`) — the
/// case where `snapshot::plan` can't see removals. When the snapshot is
/// present it already computes the remove-set, so this returns empty to avoid
/// double-reaping. Pure, so both branches are unit-tested without driving a
/// real reload.
fn orphan_removals(prev_present: bool, orphans: Vec<RemovedAgent>) -> Vec<RemovedAgent> {
    if prev_present {
        Vec::new()
    } else {
        orphans
    }
}

/// Orphans from the durable registry as drainable `RemovedAgent`s: agents it
/// records as up at this root but absent from the compose. Used only on the
/// applied.json-absent reload path, where `snapshot::plan` can't see removals
/// (T-468). The whole-tree set is returned; `filter_plan_to_project` trims it
/// to scope, mirroring how the snapshot-diff removals are handled. The tmux
/// session is the one the registry recorded (`spec_from_removed` drains by
/// session name). Best-effort: a missing or unreadable registry yields none.
fn registry_orphans(compose: &Compose) -> Vec<RemovedAgent> {
    let Some(dir) = team_core::registry::config_dir() else {
        return Vec::new();
    };
    let desired: HashSet<String> = compose.agents().map(|h| h.id()).collect();
    match team_core::registry::orphans_for_root(&dir, &compose.root, &desired, None, false) {
        Ok(orphans) => orphans
            .into_iter()
            .map(|o| RemovedAgent {
                id: o.id(),
                tmux_session: o.tmux_session,
                env_file: PathBuf::new(),
            })
            .collect(),
        Err(e) => {
            eprintln!("warn · teams registry: {e:#}");
            Vec::new()
        }
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

    #[test]
    fn fresh_suffix_annotates_only_when_fresh() {
        // The `(fresh)` annotation must compose after `(forced)` and
        // before the drain suffix in the reload log lines, and vanish
        // entirely on a non-fresh reload.
        assert_eq!(super::super::up::fresh_suffix(true), " (fresh)");
        assert_eq!(super::super::up::fresh_suffix(false), "");
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

    // ── T-384: reload --force ────────────────────────────────────────

    fn snapshot_with(ids: &[&str]) -> snapshot::Snapshot {
        let mut agents = BTreeMap::new();
        for id in ids {
            agents.insert((*id).to_string(), entry("/tmp/x.env"));
        }
        snapshot::Snapshot {
            agents,
            ..Default::default()
        }
    }

    #[test]
    fn force_promote_keeps_moves_keeps_into_change_as_forced() {
        // `--force` drags every kept (unchanged) agent into the change
        // set with the forced marker, and pulls its prior entry into
        // change_prior so teardown targets the actually-running session.
        let prev = snapshot_with(&["a:m", "a:w"]);
        let mut plan = ReloadPlan {
            keep: vec!["a:m".into(), "a:w".into()],
            ..ReloadPlan::default()
        };
        force_promote_keeps(&mut plan, Some(&prev));
        assert!(plan.keep.is_empty(), "keeps drained into change");
        assert_eq!(plan.change.len(), 2);
        for (id, inputs) in &plan.change {
            assert!(!inputs.any(), "{id} promoted as forced (all-false)");
            assert!(
                plan.change_prior.contains_key(id),
                "prior carried for {id} so teardown hits the running session"
            );
        }
        // The shared output head renders a promoted keep as the forced line.
        assert_eq!(
            change_line_head(&plan.change[0].0, &plan.change[0].1),
            "reloaded · a:m (forced)"
        );
    }

    #[test]
    fn force_promote_keeps_without_prior_entry_leaves_it_kept() {
        // Defensive: a kept id absent from the prior snapshot (can't
        // happen in practice, since keeps come from prev ∩ next) stays
        // in keep rather than landing in change with no teardown target.
        let prev = snapshot_with(&["a:m"]);
        let mut plan = ReloadPlan {
            keep: vec!["a:ghost".into()],
            ..ReloadPlan::default()
        };
        force_promote_keeps(&mut plan, Some(&prev));
        assert_eq!(plan.keep, vec!["a:ghost"]);
        assert!(plan.change.is_empty());
    }

    #[test]
    fn change_line_head_distinguishes_diff_from_forced() {
        // A genuine diff entry reports which inputs changed; a forced
        // (all-false) entry reports `reloaded · id (forced)`.
        let genuine = ChangedInputs {
            env: true,
            mcp: true,
            role_prompt: false,
        };
        assert_eq!(change_line_head("a:m", &genuine), "changed · a:m (env+mcp)");
        assert_eq!(
            change_line_head("a:m", &ChangedInputs::forced()),
            "reloaded · a:m (forced)"
        );
    }

    #[test]
    fn orphan_removals_only_fires_without_a_prior_snapshot() {
        // applied.json present (`prev` is Some) → the snapshot diff owns
        // removals, so the registry fallback must stay empty (no double-reap).
        assert!(orphan_removals(true, vec![removed("a:gone")]).is_empty());
        // applied.json absent → the registry fallback supplies the orphans
        // verbatim so `apply_plan` drains them.
        let passed = orphan_removals(false, vec![removed("a:gone"), removed("a:zap")]);
        assert_eq!(
            passed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a:gone", "a:zap"]
        );
    }
}
