//! Applied-state snapshot (`state/applied.json`) — schema v2.
//!
//! The snapshot is the single source of truth for "what was applied to
//! this teamctl root, last time `up` or `reload` ran". It is consumed by
//! `reload` to compute the diff against the current compose, and by
//! teardown paths to know the *actual* tmux session names that were
//! started — critical when global config (notably `tmux_prefix`) has
//! drifted since the last apply.
//!
//! Schema v1 (legacy `{ agents: { id -> opaque-hash } }`) is treated as
//! "no prior snapshot", which forces a clean re-apply on first reload
//! after upgrade. That one-time mass-restart is the priced-in cost of
//! moving to deterministic, content-stable fingerprints.
//!
//! Hashing is `blake3` throughout — byte-stable across builds and
//! toolchains, fixing the silent `applied.json`-invalidation that the
//! old `DefaultHasher` introduced on every Rust upgrade.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use team_core::compose::Compose;
use team_core::render::{env_path, render_agent};

pub const SCHEMA_VERSION: u32 = 2;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub tmux_session: String,
    pub env_file: String,
    pub fingerprints: Fingerprints,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprints {
    pub env: String,
    pub mcp: String,
    pub role_prompt: PromptFingerprint,
}

/// `role_prompt` is a sum type so a missing file produces a stable
/// fingerprint distinct from "no role_prompt configured" and from any
/// present file. Hiding a missing path behind empty bytes (the prior
/// behaviour) silently masked typo'd paths and deleted-underneath
/// regressions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptFingerprint {
    None,
    Missing { path: String },
    Present { hash: String },
}

/// Snapshot path on disk.
pub fn snapshot_path(root: &Path) -> PathBuf {
    root.join("state/applied.json")
}

/// Read the previously-applied snapshot. Returns `None` when:
/// - the file does not exist (first apply on this root),
/// - the file is unparseable (corrupted), or
/// - the file is schema v1 (the legacy `{ agents: { id -> hash } }`).
///
/// In all three cases the next reload will treat every current agent as
/// `add` and produce no `remove` entries — equivalent to the pre-v2
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
pub fn compute(compose: &Compose, team_mcp_bin: &str) -> Snapshot {
    let mut agents = BTreeMap::new();
    for h in compose.agents() {
        let (env, mcp) = render_agent(compose, h, team_mcp_bin);
        let role_prompt = fingerprint_role_prompt(compose, h.spec.role_prompt.as_deref());
        let fingerprints = Fingerprints {
            env: hash_str(&env),
            mcp: hash_str(&mcp),
            role_prompt,
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

fn fingerprint_role_prompt(compose: &Compose, role_prompt: Option<&Path>) -> PromptFingerprint {
    let Some(rel) = role_prompt else {
        return PromptFingerprint::None;
    };
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

fn hash_str(s: &str) -> String {
    hash_bytes(s.as_bytes())
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// What changed for a single kept agent. All-false is a `keep` (not in
/// the `change` list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedInputs {
    pub env: bool,
    pub mcp: bool,
    pub role_prompt: bool,
}

impl ChangedInputs {
    pub fn any(&self) -> bool {
        self.env || self.mcp || self.role_prompt
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

    let prev_agents: &BTreeMap<String, AgentEntry> = match prev {
        Some(s) => &s.agents,
        None => {
            // No prior snapshot: every current agent is `add`. No `remove`.
            for id in next.agents.keys() {
                plan.add.push(id.clone());
            }
            return plan;
        }
    };

    for (id, next_entry) in &next.agents {
        match prev_agents.get(id) {
            None => plan.add.push(id.clone()),
            Some(prev_entry) => {
                let inputs = ChangedInputs {
                    env: prev_entry.fingerprints.env != next_entry.fingerprints.env,
                    mcp: prev_entry.fingerprints.mcp != next_entry.fingerprints.mcp,
                    role_prompt: prev_entry.fingerprints.role_prompt
                        != next_entry.fingerprints.role_prompt,
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
        };
        assert_eq!(c.label(), "env+role_prompt");
    }

    #[test]
    fn blake3_hash_is_deterministic() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("hello "));
        assert!(hash_str("x").starts_with("blake3:"));
    }
}
