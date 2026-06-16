//! Applied-state snapshot (`state/applied.json`) — schema v3.
//!
//! The snapshot is the single source of truth for "what was applied to
//! this teamctl root, last time `up` or `reload` ran". It is consumed by
//! `reload` to compute the diff against the current compose, and by
//! teardown paths to know the *actual* tmux session names that were
//! started — critical when global config (notably `tmux_prefix`) has
//! drifted since the last apply.
//!
//! Any older schema (v1 legacy `{ agents: { id -> opaque-hash } }`, or v2
//! before per-agent hooks/subagents/skills and the teamctl-version global
//! joined the fingerprint set) is treated as "no prior snapshot", which
//! forces a clean re-apply on the first reload after upgrade. That
//! one-time mass-restart is the priced-in cost of a schema bump.
//!
//! Hashing is `blake3` throughout — byte-stable across builds and
//! toolchains, fixing the silent `applied.json`-invalidation that the
//! old `DefaultHasher` introduced on every Rust upgrade.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use team_core::compose::{Compose, RolePrompt};
use team_core::render::{env_path, render_agent, render_claude_settings, render_subagents};

pub const SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub applied_at: String,
    pub compose_digest: String,
    pub global: GlobalSnap,
    pub agents: BTreeMap<String, AgentEntry>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            applied_at: String::new(),
            compose_digest: String::new(),
            global: GlobalSnap::default(),
            agents: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalSnap {
    pub supervisor_type: String,
    pub tmux_prefix: String,
    pub broker_path: String,
    /// The teamctl binary version (`TEAMCTL_BUILD_VERSION`) that last
    /// applied this root. A mismatch on the next reload means a new
    /// binary is on disk (the post-`update` trap: the file was swapped
    /// but live agents still run the old wrapper/MCP/settings), so every
    /// agent is force-restarted to pick the new binary up.
    pub teamctl_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub tmux_session: String,
    pub env_file: String,
    pub fingerprints: Fingerprints,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprints {
    pub env: String,
    pub mcp: String,
    pub role_prompt: PromptFingerprint,
    /// Hash of the rendered Claude settings JSON (`render_claude_settings`)
    /// — covers per-agent `hooks:` and `ultracode:`, which previously
    /// flowed to disk but were never fingerprinted, so an edit to either
    /// didn't trigger a restart. `"none"` for runtimes without settings.
    pub settings: String,
    /// Hash of the rendered sub-agents JSON (`render_subagents`) — covers
    /// `subagents:` source content, not just the YAML path list. `"none"`
    /// when the agent declares no sub-agents.
    pub subagents: String,
    /// Hash of the agent's declared `skills:` path list. Catches
    /// add/remove/reorder; skill directory contents are not hashed.
    pub skills: String,
}

/// `role_prompt` is a sum type so a missing file produces a stable
/// fingerprint distinct from "no role_prompt configured" and from any
/// present file. Hiding a missing path behind empty bytes (the prior
/// behaviour) silently masked typo'd paths and deleted-underneath
/// regressions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptFingerprint {
    #[default]
    None,
    Missing {
        path: String,
    },
    Present {
        hash: String,
    },
}

/// Snapshot path on disk.
pub fn snapshot_path(root: &Path) -> PathBuf {
    root.join("state/applied.json")
}

/// Read the previously-applied snapshot. Returns `None` when:
/// - the file does not exist (first apply on this root),
/// - the file is unparseable (corrupted), or
/// - the file is an older schema (v1 legacy `{ agents: { id -> hash } }`,
///   or v2 before the comprehensive-detection fields landed).
///
/// In all these cases the next reload will treat every current agent as
/// `add` and produce no `remove` entries — equivalent to the prior
/// behaviour when `applied.json` was absent.
pub fn read(root: &Path) -> Option<Snapshot> {
    let path = snapshot_path(root);
    let raw = fs::read_to_string(&path).ok()?;
    let parsed: Snapshot = serde_json::from_str(&raw).ok()?;
    if parsed.schema == SCHEMA_VERSION {
        Some(parsed)
    } else {
        None
    }
}

/// T-133: merge a freshly-computed snapshot's per-project entries into
/// a prior snapshot. Used by scoped `up` and `reload` so `applied.json`
/// carries the named project's current fingerprints without
/// overwriting other projects' last-applied state. Other projects'
/// agent entries pass through from `prior` unchanged; entries belonging
/// to the named project that exist in `prior` but not `next` (project
/// rename or removal) are dropped. Top-level metadata is taken from
/// `next` because it reflects the YAML state the operator just looked
/// at — the next unscoped reload still re-diffs other projects'
/// agents against their stale fingerprints, so correctness holds.
pub fn merge_project_into(prior: Option<&Snapshot>, next: &Snapshot, project_id: &str) -> Snapshot {
    let prefix = format!("{project_id}:");
    let mut agents: BTreeMap<String, AgentEntry> =
        prior.map(|s| s.agents.clone()).unwrap_or_default();
    agents.retain(|id, _| !id.starts_with(&prefix));
    for (id, entry) in &next.agents {
        if id.starts_with(&prefix) {
            agents.insert(id.clone(), entry.clone());
        }
    }
    // Global metadata reflects the YAML the operator just looked at, so it
    // comes from `next` — EXCEPT `teamctl_version`. A scoped op only
    // restarts one project; advancing the version here would make the next
    // UNSCOPED reload believe the whole fleet is already on the new binary,
    // so the projects this scoped run didn't touch would never restart
    // after a `teamctl update` (T-493). Carry the prior version forward so
    // that binary-swap signal survives until an unscoped reload bounces
    // everyone. (No prior = first apply = nothing stale, so use next's.)
    let mut global = next.global.clone();
    if let Some(p) = prior {
        global.teamctl_version = p.global.teamctl_version.clone();
    }
    Snapshot {
        schema: SCHEMA_VERSION,
        applied_at: next.applied_at.clone(),
        compose_digest: next.compose_digest.clone(),
        global,
        agents,
    }
}

/// Persist the snapshot to disk, creating parent dirs as needed.
pub fn write(root: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create state/ dir")?;
    }
    let serialized = serde_json::to_string_pretty(snapshot).context("serialize snapshot")?;
    fs::write(&path, serialized).context("write applied.json")?;
    Ok(())
}

/// Compute a fresh snapshot from the live compose. The `applied_at` is
/// stamped with RFC3339 UTC. Caller decides whether to persist it (via
/// `write`) — `up` and `reload` both do, but only after their
/// respective side effects have run successfully.
pub fn compute(compose: &Compose, team_mcp_bin: &str, teamctl_version: &str) -> Snapshot {
    let mut agents = BTreeMap::new();
    for h in compose.agents() {
        let (env, mcp) = render_agent(compose, h, team_mcp_bin);
        let role_prompt = fingerprint_role_prompt(compose, h.spec.role_prompt.as_ref());
        let fingerprints = Fingerprints {
            env: hash_str(&env),
            mcp: hash_str(&mcp),
            role_prompt,
            // Hooks + ultracode ride the rendered settings JSON.
            settings: hash_opt(render_claude_settings(compose, h)),
            // A read/parse error inside render_subagents collapses to
            // "no subagents" for the fingerprint; the real render step on
            // apply surfaces the error and aborts before any restart.
            subagents: hash_opt(render_subagents(compose, h).ok().flatten()),
            // Skills has no "absent" state — an agent with none has an
            // empty list — so it hashes the (possibly empty) path key
            // directly rather than the `hash_opt` "none" sentinel.
            skills: hash_str(&skills_key(&h.spec.skills)),
        };
        let tmux_session = format!(
            "{}{}-{}",
            compose.global.supervisor.tmux_prefix, h.project, h.agent
        );
        let env_file = env_path(&compose.root, h.project, h.agent)
            .display()
            .to_string();
        agents.insert(
            h.id(),
            AgentEntry {
                tmux_session,
                env_file,
                fingerprints,
            },
        );
    }

    Snapshot {
        schema: SCHEMA_VERSION,
        applied_at: now_rfc3339(),
        compose_digest: compose_digest(compose),
        global: GlobalSnap {
            supervisor_type: compose.global.supervisor.r#type.clone(),
            tmux_prefix: compose.global.supervisor.tmux_prefix.clone(),
            broker_path: compose.global.broker.path.display().to_string(),
            teamctl_version: teamctl_version.to_string(),
        },
        agents,
    }
}

/// Hash on-disk `team-compose.yaml` bytes. Used for the fast-path "no
/// changes anywhere" check. Falls back to an empty string when the file
/// can't be read (which would mean the validate step before us failed
/// already, so this is defensive only).
fn compose_digest(compose: &Compose) -> String {
    let manifest = compose.root.join("team-compose.yaml");
    match fs::read(&manifest) {
        Ok(bytes) => hash_bytes(&bytes),
        Err(_) => String::new(),
    }
}

fn fingerprint_role_prompt(
    compose: &Compose,
    role_prompt: Option<&RolePrompt>,
) -> PromptFingerprint {
    let Some(rp) = role_prompt else {
        return PromptFingerprint::None;
    };
    match rp {
        // Single arm is byte-for-byte the legacy hash so existing
        // single-form fingerprints survive the upgrade. Without this,
        // the next up/reload after this lands forces a fresh CC
        // session for every agent that has a `role_prompt`.
        RolePrompt::Single(rel) => {
            let abs = compose.root.join(rel);
            match fs::read(&abs) {
                Ok(bytes) => PromptFingerprint::Present {
                    hash: hash_bytes(&bytes),
                },
                Err(_) => PromptFingerprint::Missing {
                    path: rel.display().to_string(),
                },
            }
        }
        // Multi arm length-prefixes every source so a split/join
        // across the file boundary produces a distinct hash. No prior
        // fingerprint exists for this shape, so back-compat is moot.
        RolePrompt::Multiple(paths) => {
            let mut hasher = blake3::Hasher::new();
            for rel in paths {
                let abs = compose.root.join(rel);
                let bytes = match fs::read(&abs) {
                    Ok(b) => b,
                    Err(_) => {
                        return PromptFingerprint::Missing {
                            path: rel.display().to_string(),
                        };
                    }
                };
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            PromptFingerprint::Present {
                hash: format!("blake3:{}", hasher.finalize().to_hex()),
            }
        }
    }
}

fn hash_str(s: &str) -> String {
    hash_bytes(s.as_bytes())
}

/// Hash an optional rendered artifact. `None` (a runtime with no settings
/// file, or an agent with no sub-agents) gets a fixed sentinel distinct
/// from any real `blake3:` hash, so "no artifact" is itself a stable,
/// comparable fingerprint rather than colliding with empty content.
fn hash_opt(v: Option<String>) -> String {
    match v {
        Some(s) => hash_str(&s),
        None => "none".to_string(),
    }
}

/// Stable key for an agent's declared `skills:`: the newline-joined
/// relative paths. Catches add/remove/reorder of skill entries. Skill
/// directory *contents* are intentionally not hashed (a `SKILL.md` edit
/// isn't detected) — the path list is the cheap, common-case signal.
fn skills_key(skills: &[PathBuf]) -> String {
    skills
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// What changed for a single kept agent. All-false is a `keep` (not in
/// the `change` list). `binary` is global (the teamctl version differs):
/// it's set on every agent at once, so a binary swap restarts the fleet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedInputs {
    pub env: bool,
    pub mcp: bool,
    pub role_prompt: bool,
    pub settings: bool,
    pub subagents: bool,
    pub skills: bool,
    pub binary: bool,
}

impl ChangedInputs {
    /// The marker a `reload --force` uses to drag an unchanged ("keep")
    /// agent into the restart set. Every input is false, so `any()` is
    /// false. `plan()` never emits this shape (it only records a
    /// `change` entry when an input actually differs), so an all-false
    /// `change` entry can only mean "forced" — which the preview/apply
    /// output renders as `(forced)` instead of a changed-inputs label.
    pub fn forced() -> Self {
        Self::default()
    }

    pub fn any(&self) -> bool {
        self.env
            || self.mcp
            || self.role_prompt
            || self.settings
            || self.subagents
            || self.skills
            || self.binary
    }

    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.env {
            parts.push("env");
        }
        if self.mcp {
            parts.push("mcp");
        }
        if self.role_prompt {
            parts.push("role_prompt");
        }
        if self.settings {
            parts.push("settings");
        }
        if self.subagents {
            parts.push("subagents");
        }
        if self.skills {
            parts.push("skills");
        }
        if self.binary {
            parts.push("binary");
        }
        parts.join("+")
    }
}

/// Identifies an agent that exists in the prior snapshot but not the
/// next. Carries the *prior* tmux session name and env-file path so
/// teardown is correct even when `global` config (`tmux_prefix` etc.)
/// has changed since the last apply.
#[derive(Debug, Clone)]
pub struct RemovedAgent {
    pub id: String,
    pub tmux_session: String,
    pub env_file: PathBuf,
}

/// First-class restart plan, computed once from prev/next snapshots and
/// consumed both by `--dry-run` (PR B) and by the apply path. Sharing
/// the structure means preview and apply cannot drift.
#[derive(Debug, Default)]
pub struct ReloadPlan {
    pub add: Vec<String>,
    pub change: Vec<(String, ChangedInputs)>,
    pub remove: Vec<RemovedAgent>,
    pub keep: Vec<String>,
    /// Carries the *prior* AgentEntry for ids in `change` so the
    /// teardown side of the restart targets the actually-running tmux
    /// session, not a freshly-reconstructed one.
    pub change_prior: BTreeMap<String, AgentEntry>,
}

impl ReloadPlan {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.change.is_empty() && self.remove.is_empty()
    }
}

pub fn plan(prev: Option<&Snapshot>, next: &Snapshot) -> ReloadPlan {
    let mut plan = ReloadPlan::default();

    let prev_snap = match prev {
        Some(s) => s,
        None => {
            // No prior snapshot: every current agent is `add`. No `remove`.
            for id in next.agents.keys() {
                plan.add.push(id.clone());
            }
            return plan;
        }
    };
    let prev_agents = &prev_snap.agents;

    // A teamctl-binary swap is global: it forces every kept agent into the
    // change set so the whole fleet restarts onto the new binary's
    // wrapper/MCP/settings. This is the post-`update` trap — the on-disk
    // file changed but live agents still run the old one. Added/removed
    // agents are already handled by their own arms below.
    let binary_changed = prev_snap.global.teamctl_version != next.global.teamctl_version;

    for (id, next_entry) in &next.agents {
        match prev_agents.get(id) {
            None => plan.add.push(id.clone()),
            Some(prev_entry) => {
                let inputs = ChangedInputs {
                    env: prev_entry.fingerprints.env != next_entry.fingerprints.env,
                    mcp: prev_entry.fingerprints.mcp != next_entry.fingerprints.mcp,
                    role_prompt: prev_entry.fingerprints.role_prompt
                        != next_entry.fingerprints.role_prompt,
                    settings: prev_entry.fingerprints.settings != next_entry.fingerprints.settings,
                    subagents: prev_entry.fingerprints.subagents
                        != next_entry.fingerprints.subagents,
                    skills: prev_entry.fingerprints.skills != next_entry.fingerprints.skills,
                    binary: binary_changed,
                };
                if inputs.any() {
                    plan.change.push((id.clone(), inputs));
                    plan.change_prior.insert(id.clone(), prev_entry.clone());
                } else {
                    plan.keep.push(id.clone());
                }
            }
        }
    }

    for (id, prev_entry) in prev_agents {
        if !next.agents.contains_key(id) {
            plan.remove.push(RemovedAgent {
                id: id.clone(),
                tmux_session: prev_entry.tmux_session.clone(),
                env_file: PathBuf::from(&prev_entry.env_file),
            });
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(env: &str, mcp: &str, prompt: PromptFingerprint) -> Fingerprints {
        Fingerprints {
            env: env.into(),
            mcp: mcp.into(),
            role_prompt: prompt,
            ..Default::default()
        }
    }

    fn entry(session: &str, fp: Fingerprints) -> AgentEntry {
        AgentEntry {
            tmux_session: session.into(),
            env_file: format!("envs/{session}.env"),
            fingerprints: fp,
        }
    }

    fn snap(agents: Vec<(&str, AgentEntry)>) -> Snapshot {
        let mut map = BTreeMap::new();
        for (k, v) in agents {
            map.insert(k.into(), v);
        }
        Snapshot {
            schema: SCHEMA_VERSION,
            applied_at: "2026-04-30T00:00:00Z".into(),
            compose_digest: "blake3:test".into(),
            global: GlobalSnap::default(),
            agents: map,
        }
    }

    #[test]
    fn no_prior_marks_all_as_add() {
        let next = snap(vec![(
            "p:a",
            entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
        )]);
        let p = plan(None, &next);
        assert_eq!(p.add, vec!["p:a"]);
        assert!(p.change.is_empty());
        assert!(p.remove.is_empty());
        assert!(p.keep.is_empty());
    }

    #[test]
    fn identical_snapshots_are_all_keep() {
        let s = snap(vec![(
            "p:a",
            entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
        )]);
        let p = plan(Some(&s), &s);
        assert!(p.is_empty());
        assert_eq!(p.keep, vec!["p:a"]);
    }

    #[test]
    fn env_change_only_labels_env() {
        let prev = snap(vec![(
            "p:a",
            entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
        )]);
        let next = snap(vec![(
            "p:a",
            entry("a-p-a", fp("e2", "m1", PromptFingerprint::None)),
        )]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.change.len(), 1);
        assert_eq!(p.change[0].1.label(), "env");
    }

    #[test]
    fn role_prompt_missing_vs_none_distinct() {
        let none = PromptFingerprint::None;
        let missing = PromptFingerprint::Missing {
            path: "roles/x.md".into(),
        };
        assert_ne!(none, missing);
    }

    #[test]
    fn removal_carries_prior_tmux_session() {
        let prev = snap(vec![(
            "p:a",
            entry("OLD-p-a", fp("e1", "m1", PromptFingerprint::None)),
        )]);
        let next = snap(vec![]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.remove.len(), 1);
        assert_eq!(p.remove[0].id, "p:a");
        assert_eq!(p.remove[0].tmux_session, "OLD-p-a");
    }

    #[test]
    fn change_carries_prior_entry_for_safe_teardown() {
        let prev = snap(vec![(
            "p:a",
            entry("OLD-p-a", fp("e1", "m1", PromptFingerprint::None)),
        )]);
        let next = snap(vec![(
            "p:a",
            entry("NEW-p-a", fp("e2", "m1", PromptFingerprint::None)),
        )]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.change.len(), 1);
        assert_eq!(p.change_prior.get("p:a").unwrap().tmux_session, "OLD-p-a");
    }

    // ── T-493: comprehensive change detection ────────────────────────

    fn snap_ver(agents: Vec<(&str, AgentEntry)>, version: &str) -> Snapshot {
        let mut s = snap(agents);
        s.global.teamctl_version = version.into();
        s
    }

    #[test]
    fn binary_version_mismatch_restarts_all_kept_agents() {
        // A teamctl binary swap (the global version differs) forces every
        // otherwise-unchanged agent into the change set with the `binary`
        // input set, so the whole fleet restarts onto the new binary's
        // wrapper/MCP/settings. This is the post-`update` trap.
        let agents = || {
            vec![
                (
                    "p:a",
                    entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
                ),
                (
                    "p:b",
                    entry("a-p-b", fp("e2", "m2", PromptFingerprint::None)),
                ),
            ]
        };
        let prev = snap_ver(agents(), "0.10.0");
        let next = snap_ver(agents(), "0.11.0");
        let p = plan(Some(&prev), &next);
        assert!(
            p.keep.is_empty(),
            "no agent stays kept across a binary swap"
        );
        assert_eq!(p.change.len(), 2);
        for (id, inputs) in &p.change {
            assert!(inputs.binary, "{id} flagged binary");
            assert_eq!(
                inputs.label(),
                "binary",
                "{id} restarted solely for the binary swap"
            );
            assert!(
                p.change_prior.contains_key(id),
                "prior carried for {id} so teardown hits the running session"
            );
        }
    }

    #[test]
    fn same_binary_version_leaves_unchanged_agents_kept() {
        let prev = snap_ver(
            vec![(
                "p:a",
                entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
            )],
            "0.10.0",
        );
        let next = snap_ver(
            vec![(
                "p:a",
                entry("a-p-a", fp("e1", "m1", PromptFingerprint::None)),
            )],
            "0.10.0",
        );
        let p = plan(Some(&prev), &next);
        assert_eq!(p.keep, vec!["p:a"]);
        assert!(p.change.is_empty());
    }

    #[test]
    fn settings_change_flags_settings() {
        // hooks / ultracode ride the rendered settings JSON.
        let prev = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    settings: "blake3:s1".into(),
                    ..Default::default()
                },
            ),
        )]);
        let next = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    settings: "blake3:s2".into(),
                    ..Default::default()
                },
            ),
        )]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.change.len(), 1);
        assert_eq!(p.change[0].1.label(), "settings");
    }

    #[test]
    fn subagents_change_flags_subagents() {
        let prev = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    subagents: "blake3:x".into(),
                    ..Default::default()
                },
            ),
        )]);
        let next = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    subagents: "blake3:y".into(),
                    ..Default::default()
                },
            ),
        )]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.change[0].1.label(), "subagents");
    }

    #[test]
    fn skills_change_flags_skills() {
        let prev = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    skills: "blake3:k1".into(),
                    ..Default::default()
                },
            ),
        )]);
        let next = snap(vec![(
            "p:a",
            entry(
                "a-p-a",
                Fingerprints {
                    skills: "blake3:k2".into(),
                    ..Default::default()
                },
            ),
        )]);
        let p = plan(Some(&prev), &next);
        assert_eq!(p.change[0].1.label(), "skills");
    }

    #[test]
    fn binary_label_combines_with_other_inputs() {
        let c = ChangedInputs {
            env: true,
            binary: true,
            ..Default::default()
        };
        assert_eq!(c.label(), "env+binary");
    }

    #[test]
    fn old_schema_v2_applied_json_is_rejected_as_no_prior() {
        // A real v2 applied.json — schema 2, global WITHOUT
        // teamctl_version, fingerprints WITHOUT settings/subagents/skills
        // — must be treated as no-prior after the v3 bump, forcing the
        // one-time full re-apply. Mirrors schema_v1_is_treated_as_no_prior
        // but for the v2→v3 step, and exercises the realistic upgrade path
        // (deserialization fails on the missing fields) rather than just
        // the schema gate.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        let v2_raw = r#"{
            "schema": 2,
            "applied_at": "2026-06-01T00:00:00Z",
            "compose_digest": "blake3:abc",
            "global": {"supervisor_type":"tmux","tmux_prefix":"a-","broker_path":"state/mailbox.db"},
            "agents": {"p:a": {"tmux_session":"a-p-a","env_file":"state/envs/p-a.env","fingerprints":{"env":"blake3:e","mcp":"blake3:m","role_prompt":{"kind":"none"}}}}
        }"#;
        std::fs::write(snapshot_path(dir.path()), v2_raw).unwrap();
        assert!(
            read(dir.path()).is_none(),
            "a real v2 applied.json must be no-prior after the v3 bump"
        );
    }

    #[test]
    fn scoped_merge_keeps_prior_binary_version() {
        // T-493 finding: a scoped reload/up after `teamctl update` must NOT
        // advance the global teamctl_version — otherwise the next unscoped
        // reload thinks the whole fleet is already on the new binary and
        // never bounces the projects the scoped run skipped.
        let prior = snap_ver(
            vec![("a:m", entry("a-a-m", fp("e", "m", PromptFingerprint::None)))],
            "0.10.0",
        );
        let next = snap_ver(
            vec![("a:m", entry("a-a-m", fp("e", "m", PromptFingerprint::None)))],
            "0.11.0",
        );
        let merged = merge_project_into(Some(&prior), &next, "a");
        assert_eq!(
            merged.global.teamctl_version, "0.10.0",
            "scoped merge carries the prior binary version forward"
        );
        // ...so a subsequent unscoped reload still detects the swap and
        // flags every agent with `binary`.
        let p = plan(Some(&merged), &next);
        assert!(
            p.change.iter().all(|(_, i)| i.binary),
            "binary swap still detected after a scoped merge"
        );
        assert!(p.keep.is_empty());
    }

    #[test]
    fn hash_opt_distinguishes_none_from_content() {
        // "no artifact" must be a stable sentinel that never collides with
        // a real blake3 hash of empty or any content.
        assert_eq!(hash_opt(None), "none");
        assert_ne!(hash_opt(Some(String::new())), hash_opt(None));
        assert!(hash_opt(Some("x".into())).starts_with("blake3:"));
    }

    #[test]
    fn schema_v1_is_treated_as_no_prior() {
        let v1_raw = r#"{"agents":{"p:a":"deadbeef"}}"#;
        let parsed: Result<Snapshot, _> = serde_json::from_str(v1_raw);
        // serde_json with the new schema rejects v1 because `schema`
        // field is missing. read() catches the parse failure and
        // returns None, which downstream code treats as "no prior".
        assert!(parsed.is_err());
    }

    #[test]
    fn fingerprint_label_combines_inputs() {
        let c = ChangedInputs {
            env: true,
            mcp: false,
            role_prompt: true,
            ..Default::default()
        };
        assert_eq!(c.label(), "env+role_prompt");
    }

    #[test]
    fn forced_changed_inputs_is_all_false() {
        // T-384: `--force` drags unchanged agents into the change set
        // carrying this all-false marker. `any()` must be false so the
        // apply/preview output recognises it as forced (not a real diff),
        // and so it can never be confused with a genuine plan() change
        // entry, which is only emitted when an input actually differs.
        let f = ChangedInputs::forced();
        assert!(!f.env && !f.mcp && !f.role_prompt);
        assert!(!f.any());
        assert_eq!(f.label(), "");
    }

    #[test]
    fn blake3_hash_is_deterministic() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("hello "));
        assert!(hash_str("x").starts_with("blake3:"));
    }

    // ── T-133 merge_project_into ───────────────────────────────────

    #[test]
    fn merge_replaces_named_project_entries_and_keeps_others() {
        // Scoped up of project `a` against a prior snapshot that
        // covers `a` and `b`. Result: `a`'s entries reflect `next`'s
        // (current) fingerprints; `b`'s entries are untouched.
        let prior = snap(vec![
            (
                "a:m",
                entry("a-a-m", fp("old-env", "old-mcp", PromptFingerprint::None)),
            ),
            (
                "b:m",
                entry("a-b-m", fp("b-env", "b-mcp", PromptFingerprint::None)),
            ),
        ]);
        let next = snap(vec![
            (
                "a:m",
                entry("a-a-m", fp("new-env", "new-mcp", PromptFingerprint::None)),
            ),
            (
                "b:m",
                entry(
                    "a-b-m",
                    fp("b-changed-env", "b-mcp", PromptFingerprint::None),
                ),
            ),
        ]);
        let merged = merge_project_into(Some(&prior), &next, "a");
        assert_eq!(merged.agents["a:m"].fingerprints.env, "new-env");
        // b's entry is taken from prior, not next — scoped run does
        // NOT carry forward project b's recomputed fingerprints.
        assert_eq!(merged.agents["b:m"].fingerprints.env, "b-env");
    }

    #[test]
    fn merge_drops_named_project_entries_present_in_prior_but_not_next() {
        // A worker was renamed/removed inside project `a`. The
        // scoped-merged snapshot drops the stale entry so the next
        // unscoped reload doesn't try to teardown an agent the
        // current YAML no longer defines.
        let prior = snap(vec![
            (
                "a:gone",
                entry("a-a-gone", fp("e", "m", PromptFingerprint::None)),
            ),
            (
                "b:keep",
                entry("a-b-keep", fp("e", "m", PromptFingerprint::None)),
            ),
        ]);
        let next = snap(vec![(
            "a:m",
            entry("a-a-m", fp("e", "m", PromptFingerprint::None)),
        )]);
        let merged = merge_project_into(Some(&prior), &next, "a");
        assert!(!merged.agents.contains_key("a:gone"));
        assert!(merged.agents.contains_key("a:m"));
        assert!(merged.agents.contains_key("b:keep"));
    }

    #[test]
    fn merge_with_no_prior_falls_back_to_next_filtered() {
        // First-ever scoped run on a fresh root: applied.json is
        // absent, so prior is None. The merged snapshot ends up with
        // only the named project's entries from `next`. Subsequent
        // unscoped reload re-renders the rest.
        let next = snap(vec![
            ("a:m", entry("a-a-m", fp("e", "m", PromptFingerprint::None))),
            ("b:m", entry("a-b-m", fp("e", "m", PromptFingerprint::None))),
        ]);
        let merged = merge_project_into(None, &next, "a");
        assert!(merged.agents.contains_key("a:m"));
        assert!(!merged.agents.contains_key("b:m"));
    }

    #[test]
    fn merge_uses_next_top_level_metadata() {
        // compose_digest, global, applied_at always come from `next`
        // — they reflect the YAML state the operator just looked at.
        // Other-project per-agent fingerprints stay at prior values
        // (re-diffed by the next unscoped reload), so correctness
        // holds without needing two separate digest fields.
        let prior = Snapshot {
            applied_at: "old-time".into(),
            compose_digest: "blake3:old".into(),
            global: GlobalSnap {
                tmux_prefix: "old-".into(),
                ..GlobalSnap::default()
            },
            agents: BTreeMap::new(),
            ..Snapshot::default()
        };
        let next = Snapshot {
            applied_at: "new-time".into(),
            compose_digest: "blake3:new".into(),
            global: GlobalSnap {
                tmux_prefix: "new-".into(),
                ..GlobalSnap::default()
            },
            agents: BTreeMap::new(),
            ..Snapshot::default()
        };
        let merged = merge_project_into(Some(&prior), &next, "a");
        assert_eq!(merged.applied_at, "new-time");
        assert_eq!(merged.compose_digest, "blake3:new");
        assert_eq!(merged.global.tmux_prefix, "new-");
    }

    #[test]
    fn merge_does_not_touch_prefix_collision_projects() {
        // `aa:m` does not start with `a:` — scoped merge for project
        // `a` must leave `aa`'s entries untouched.
        let prior = snap(vec![
            (
                "a:m",
                entry("a-a-m", fp("a-env", "m", PromptFingerprint::None)),
            ),
            (
                "aa:m",
                entry("a-aa-m", fp("aa-env", "m", PromptFingerprint::None)),
            ),
        ]);
        let next = snap(vec![(
            "a:m",
            entry("a-a-m", fp("new-env", "m", PromptFingerprint::None)),
        )]);
        let merged = merge_project_into(Some(&prior), &next, "a");
        assert_eq!(merged.agents["a:m"].fingerprints.env, "new-env");
        assert_eq!(merged.agents["aa:m"].fingerprints.env, "aa-env");
    }

    /// Smallest possible Compose for fingerprint tests: one project, one
    /// manager, no channels. Fingerprint code only reads `compose.root`
    /// and the agent's `role_prompt`.
    fn compose_with_root(root: &Path) -> Compose {
        use std::collections::BTreeMap;
        use team_core::compose::*;
        let mut managers = BTreeMap::new();
        managers.insert(
            "mgr".into(),
            Agent {
                runtime: "claude-code".into(),
                model: None,
                role_prompt: None,
                permission_mode: None,
                autonomy: "low_risk_only".into(),
                can_dm: vec![],
                can_broadcast: vec![],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
                ultracode: false,
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
                skills: vec![],
            },
        );
        Compose {
            root: root.to_path_buf(),
            global: Global {
                version: team_core::compose::SchemaVersion::new("2.0.0"),
                broker: Default::default(),
                supervisor: Default::default(),
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
                attachments: Default::default(),
            },
            projects: vec![Project {
                version: 2,
                project: ProjectMeta {
                    id: "p".into(),
                    name: "P".into(),
                    cwd: root.to_path_buf(),
                },
                channels: vec![],
                managers,
                workers: Default::default(),
                interfaces: None,
            }],
        }
    }

    #[test]
    fn fingerprint_multifile_flips_on_any_source_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        std::fs::write(root.join("roles/_base.md"), "BASE").unwrap();
        std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

        let c = compose_with_root(root);
        let rp = RolePrompt::Multiple(vec![
            PathBuf::from("roles/_base.md"),
            PathBuf::from("roles/mgr.md"),
        ]);
        let before = fingerprint_role_prompt(&c, Some(&rp));

        std::fs::write(root.join("roles/_base.md"), "BASE-v2").unwrap();
        let after = fingerprint_role_prompt(&c, Some(&rp));
        assert_ne!(before, after);
    }

    #[test]
    fn fingerprint_multifile_flips_on_reorder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        std::fs::write(root.join("roles/a.md"), "A").unwrap();
        std::fs::write(root.join("roles/b.md"), "B").unwrap();

        let c = compose_with_root(root);
        let ab = RolePrompt::Multiple(vec![
            PathBuf::from("roles/a.md"),
            PathBuf::from("roles/b.md"),
        ]);
        let ba = RolePrompt::Multiple(vec![
            PathBuf::from("roles/b.md"),
            PathBuf::from("roles/a.md"),
        ]);
        assert_ne!(
            fingerprint_role_prompt(&c, Some(&ab)),
            fingerprint_role_prompt(&c, Some(&ba)),
        );
    }

    #[test]
    fn fingerprint_single_form_matches_legacy_byte_hash() {
        // Back-compat: every agent with a single-string `role_prompt`
        // already has a `blake3:<hex>` fingerprint stored in
        // applied.json. The Single arm must produce the same hash
        // those rows already hold, otherwise the first up/reload after
        // this lands force-restarts every agent in the fleet.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        let body = b"manager role copy\n".to_vec();
        std::fs::write(root.join("roles/mgr.md"), &body).unwrap();

        let c = compose_with_root(root);
        let rp = RolePrompt::Single(PathBuf::from("roles/mgr.md"));
        let got = fingerprint_role_prompt(&c, Some(&rp));
        let expected_legacy = format!("blake3:{}", blake3::hash(&body).to_hex());
        match got {
            PromptFingerprint::Present { hash } => assert_eq!(hash, expected_legacy),
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn fingerprint_multifile_missing_source_returns_missing() {
        let dir = tempfile::tempdir().unwrap();
        let c = compose_with_root(dir.path());
        let rp = RolePrompt::Multiple(vec![
            PathBuf::from("roles/present.md"),
            PathBuf::from("roles/missing.md"),
        ]);
        std::fs::create_dir_all(dir.path().join("roles")).unwrap();
        std::fs::write(dir.path().join("roles/present.md"), "P").unwrap();

        let fp = fingerprint_role_prompt(&c, Some(&rp));
        match fp {
            PromptFingerprint::Missing { path } => assert_eq!(path, "roles/missing.md"),
            other => panic!("expected Missing, got {other:?}"),
        }
    }
}
