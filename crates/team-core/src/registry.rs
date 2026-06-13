//! Durable, system-wide record of which teams are up and where they live.
//!
//! teamctl's other notions of identity are all keyed on *name*, never on
//! *path*: the session UUID, the tmux session name, the mailbox row key,
//! the `applied.json` key. So nothing could durably answer "what teams are
//! running across this machine, and in which directories?" — the question
//! behind system-wide `teamctl ps`, orphan reaping, and the same-name guard.
//!
//! This registry is that durable record. It lives at
//! `~/.config/teamctl/teams.json` (the same config dir the `context` store
//! uses) and is keyed on the compound `(project_id, root)` — one entry per
//! project per absolute team root, so two installs of the same template in
//! different folders are distinct rows rather than a collision.
//!
//! `up` upserts this team's entries (see `crates/teamctl/src/cmd/up.rs`),
//! `down` clears them (`down.rs`). Writes are atomic (temp + rename) so a
//! reader never observes a torn file. Atomicity is per-write, not a
//! cross-process lock: two `up`s of *different* teams racing the
//! load-modify-save could still drop one update — acceptable for a
//! self-healing side store (the next `up` re-records it), and called out
//! here rather than over-claimed.
//!
//! The store *records* the path alongside the name; it deliberately does
//! NOT make the session UUID or tmux name itself path-aware (a larger
//! identity change, out of scope).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// File name under the config dir.
const FILE: &str = "teams.json";

/// One running team: a single `(project_id, root)` pair plus the metadata
/// `ps` / reaping / the same-name guard need. Field names serialize verbatim
/// (snake_case), matching every other persisted teamctl store.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamEntry {
    /// Project id as declared in the compose (bare, e.g. `main`).
    #[serde(default)]
    pub project_id: String,
    /// Absolute, canonicalized team root (the directory `up` recorded —
    /// the same value tagged onto the tmux session as `@teamctl-root`).
    #[serde(default)]
    pub root: PathBuf,
    /// Supervisor tmux prefix in effect when the team came up.
    #[serde(default)]
    pub tmux_prefix: String,
    /// Sorted agent names belonging to this project.
    #[serde(default)]
    pub agents: Vec<String>,
    /// RFC3339 timestamp of when this entry was first recorded by `up`.
    /// Preserved across an idempotent re-`up` so uptime doesn't reset.
    #[serde(default)]
    pub started_at: String,
}

/// The whole store. `#[serde(default)]` per field keeps old files
/// forward-compatible as the shape grows, matching `ContextStore`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub teams: Vec<TeamEntry>,
}

/// `~/.config/teamctl` — derived from `$HOME` (`$USERPROFILE` fallback for
/// Windows). `None` when neither is set, so callers warn-and-skip rather
/// than guess a path. Mirrors the CLI `context` store's resolver and
/// `session::claude_home`; deliberately NOT `dirs::config_dir()`, which on
/// macOS resolves to `~/Library/Application Support` and would break the
/// literal `~/.config/teamctl` path the rest of the CLI uses.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".config/teamctl"))
}

/// RFC3339 (seconds, `Z`) — the serialized-timestamp idiom shared with
/// `snapshot::applied_at` / `attachments`. Co-located here because this
/// module owns the `started_at` field's format contract.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Load the registry from `dir/teams.json`. A missing file is an empty
/// registry; a corrupt one degrades to empty rather than erroring (the
/// next `up` overwrites it) — the same tolerance `ContextStore::load` and
/// `snapshot::read` apply. Only a genuine read error (e.g. permissions) is
/// propagated. `dir` is the config dir (see [`config_dir`]); it is taken as
/// a parameter so tests can point at a tempdir without touching `$HOME`.
pub fn load(dir: &Path) -> Result<Registry> {
    let path = dir.join(FILE);
    if !path.exists() {
        return Ok(Registry::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

/// Upsert one team entry. Convenience over [`upsert_many`].
pub fn upsert(dir: &Path, entry: TeamEntry) -> Result<()> {
    upsert_many(dir, vec![entry])
}

/// Upsert a batch in a single load-modify-save (one atomic write for the
/// whole `up`). Each entry replaces any existing row with the same
/// `(project_id, root)` key, preserving that row's original `started_at`;
/// new keys are appended. Entries stay sorted by `(root, project_id)` for
/// stable on-disk output.
pub fn upsert_many(dir: &Path, entries: Vec<TeamEntry>) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut reg = load(dir)?;
    for mut entry in entries {
        if let Some(existing) = reg
            .teams
            .iter_mut()
            .find(|t| t.project_id == entry.project_id && t.root == entry.root)
        {
            if !existing.started_at.is_empty() {
                entry.started_at = existing.started_at.clone();
            }
            *existing = entry;
        } else {
            reg.teams.push(entry);
        }
    }
    reg.teams.sort_by(|a, b| {
        a.root
            .cmp(&b.root)
            .then_with(|| a.project_id.cmp(&b.project_id))
    });
    save(dir, &reg)
}

/// Clear entries for `root`. `project = None` drops every entry at that
/// root (a whole-team `down`); `project = Some(id)` drops only that
/// project's entry (a project-scoped `down` on a multi-project root, so
/// sibling projects stay registered). A no-op write is skipped.
pub fn clear(dir: &Path, root: &Path, project: Option<&str>) -> Result<()> {
    let mut reg = load(dir)?;
    let before = reg.teams.len();
    reg.teams
        .retain(|t| t.root != root || project.is_some_and(|p| t.project_id != p));
    if reg.teams.len() != before {
        save(dir, &reg)?;
    }
    Ok(())
}

/// `true` when this entry's recorded root no longer holds a
/// `team-compose.yaml` — a stale row whose team config was moved or
/// deleted. `up` always records the `.team` directory as `root`, so the
/// `<root>/team-compose.yaml` check is the live one for registry entries;
/// the `<root>/.team/team-compose.yaml` fallback is kept only for parity
/// with the shared `sessions::root_has_compose` rule (which also accepts a
/// root that points at the bare project dir). `path_exists` is injected so
/// the classifier is pure and unit-testable; production passes
/// `|p| p.exists()`.
pub fn is_orphan(entry: &TeamEntry, path_exists: &impl Fn(&Path) -> bool) -> bool {
    !(path_exists(&entry.root.join("team-compose.yaml"))
        || path_exists(&entry.root.join(".team").join("team-compose.yaml")))
}

/// One recorded `(project, agent)` running under a root, with its tmux
/// session name reconstructed from the prefix the team was brought up with.
/// The unit `down`/`reload` orphan-reaping operates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    pub project_id: String,
    pub agent: String,
    pub tmux_session: String,
}

impl RosterEntry {
    /// `<project>:<agent>` — matches `compose.agents().id()`.
    pub fn id(&self) -> String {
        format!("{}:{}", self.project_id, self.agent)
    }
}

impl Registry {
    /// The recorded roster for `root`: one [`RosterEntry`] per agent of
    /// every team registered at that root. The tmux session is rebuilt from
    /// the team's recorded `tmux_prefix` (`{prefix}{project}-{agent}`, the
    /// supervisor's naming formula) so a reaped session is targeted with the
    /// name it was started under — correct even if the live compose's prefix
    /// has since drifted.
    pub fn roster_for_root(&self, root: &Path) -> Vec<RosterEntry> {
        self.teams
            .iter()
            .filter(|t| t.root == root)
            .flat_map(|t| {
                t.agents.iter().map(move |agent| RosterEntry {
                    project_id: t.project_id.clone(),
                    agent: agent.clone(),
                    tmux_session: format!("{}{}-{}", t.tmux_prefix, t.project_id, agent),
                })
            })
            .collect()
    }
}

/// The reap set: recorded roster agents that are no longer desired.
/// `desired` is the current compose's agent ids (`<project>:<agent>`).
/// `scoped` limits the reap to one project (a `--project` down/reload);
/// `per_agent` (a `--agent` selector active) disables reaping entirely — a
/// partial teardown leaves the rest of the team registered and running, so
/// nothing there is an orphan. Pure; mirrors `down::registry_clear_scope`'s
/// scope contract so reaping is never broader than the invocation.
pub fn reap_targets(
    roster: &[RosterEntry],
    desired: &std::collections::HashSet<String>,
    scoped: Option<&str>,
    per_agent: bool,
) -> Vec<RosterEntry> {
    if per_agent {
        return Vec::new();
    }
    roster
        .iter()
        .filter(|e| scoped.is_none_or(|p| e.project_id == p))
        .filter(|e| !desired.contains(&e.id()))
        .cloned()
        .collect()
}

/// Load the registry at `dir` and return the orphan roster for `root` —
/// recorded agents no longer in `desired`. The down/reload reap entry point:
/// composes [`load`] + [`Registry::roster_for_root`] + [`reap_targets`] so the
/// whole on-disk → orphans chain is one tested call. `dir` is taken as a
/// parameter so tests point at a tempdir without touching `$HOME`. A missing
/// store loads as empty (no orphans); only a genuine read error propagates.
pub fn orphans_for_root(
    dir: &Path,
    root: &Path,
    desired: &std::collections::HashSet<String>,
    scoped: Option<&str>,
    per_agent: bool,
) -> Result<Vec<RosterEntry>> {
    let reg = load(dir)?;
    Ok(reap_targets(
        &reg.roster_for_root(root),
        desired,
        scoped,
        per_agent,
    ))
}

/// If another team is registered with the same `project_id` but a DIFFERENT
/// root that still holds a compose (not an orphan), return that root — the
/// same-name-different-folder collision `up` must refuse. It's the cluster's
/// root cause: identity is name-keyed, not path-keyed, so two such teams
/// alias each other's sessions, tmux names, and mailbox keys. The team's own
/// root is never a conflict (a re-up of the same `(project_id, root)` is
/// fine). `path_exists` is injected so the liveness check is pure and
/// unit-testable; production passes `|p| p.exists()`.
pub fn same_name_other_root(
    reg: &Registry,
    project_id: &str,
    root: &Path,
    path_exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    reg.teams
        .iter()
        .find(|t| t.project_id == project_id && t.root != root && !is_orphan(t, path_exists))
        .map(|t| t.root.clone())
}

/// Atomic write: serialize to a pid-scoped sibling temp file, then rename
/// over the target so a concurrent reader sees either the old or the new
/// file, never a partial one. The pid in the temp name keeps two processes
/// from clobbering each other's temp. Mirrors the `attachments` /
/// `~/.claude.json` temp+rename idiom (no shared helper exists to reuse).
fn save(dir: &Path, reg: &Registry) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    let body = serde_json::to_string_pretty(reg)?;
    fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn entry(project: &str, root: &str, agents: &[&str], started: &str) -> TeamEntry {
        TeamEntry {
            project_id: project.into(),
            root: PathBuf::from(root),
            tmux_prefix: "t-".into(),
            agents: agents.iter().map(|s| (*s).to_string()).collect(),
            started_at: started.into(),
        }
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let reg = load(dir.path()).unwrap();
        assert!(reg.teams.is_empty());
    }

    #[test]
    fn upsert_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        upsert(
            dir.path(),
            entry(
                "main",
                "/r/a/.team",
                &["compass", "scout"],
                "2026-06-13T00:00:00Z",
            ),
        )
        .unwrap();
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.teams.len(), 1);
        let t = &reg.teams[0];
        assert_eq!(t.project_id, "main");
        assert_eq!(t.root, PathBuf::from("/r/a/.team"));
        assert_eq!(t.tmux_prefix, "t-");
        assert_eq!(t.agents, vec!["compass", "scout"]);
        assert_eq!(t.started_at, "2026-06-13T00:00:00Z");
    }

    #[test]
    fn upsert_same_key_replaces_and_preserves_started_at() {
        let dir = tempfile::tempdir().unwrap();
        upsert(dir.path(), entry("main", "/r/a/.team", &["compass"], "T0")).unwrap();
        // Re-up with a different roster + a newer (would-be) start time.
        upsert(
            dir.path(),
            entry("main", "/r/a/.team", &["compass", "scribe"], "T1"),
        )
        .unwrap();
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.teams.len(), 1, "same (project,root) is one row");
        assert_eq!(
            reg.teams[0].agents,
            vec!["compass", "scribe"],
            "roster refreshed"
        );
        assert_eq!(
            reg.teams[0].started_at, "T0",
            "uptime preserved across re-up"
        );
    }

    #[test]
    fn distinct_keys_coexist_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        // Same project id, different roots → two rows (the path-keyed fix).
        // Different project, shared root → two rows.
        upsert_many(
            dir.path(),
            vec![
                entry("main", "/r/b/.team", &["x"], "T0"),
                entry("main", "/r/a/.team", &["x"], "T0"),
                entry("ops", "/r/a/.team", &["y"], "T0"),
            ],
        )
        .unwrap();
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.teams.len(), 3);
        // Sorted by (root, project_id).
        assert_eq!(
            reg.teams
                .iter()
                .map(|t| (t.root.to_string_lossy().into_owned(), t.project_id.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("/r/a/.team".into(), "main".into()),
                ("/r/a/.team".into(), "ops".into()),
                ("/r/b/.team".into(), "main".into()),
            ]
        );
    }

    #[test]
    fn clear_whole_root_drops_all_its_entries() {
        let dir = tempfile::tempdir().unwrap();
        upsert_many(
            dir.path(),
            vec![
                entry("main", "/r/a/.team", &["x"], "T0"),
                entry("ops", "/r/a/.team", &["y"], "T0"),
                entry("main", "/r/b/.team", &["z"], "T0"),
            ],
        )
        .unwrap();
        clear(dir.path(), Path::new("/r/a/.team"), None).unwrap();
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.teams.len(), 1);
        assert_eq!(reg.teams[0].root, PathBuf::from("/r/b/.team"));
    }

    #[test]
    fn clear_scoped_project_keeps_sibling_at_same_root() {
        let dir = tempfile::tempdir().unwrap();
        upsert_many(
            dir.path(),
            vec![
                entry("main", "/r/a/.team", &["x"], "T0"),
                entry("ops", "/r/a/.team", &["y"], "T0"),
            ],
        )
        .unwrap();
        clear(dir.path(), Path::new("/r/a/.team"), Some("main")).unwrap();
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.teams.len(), 1, "only the scoped project is dropped");
        assert_eq!(reg.teams[0].project_id, "ops");
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        upsert(dir.path(), entry("main", "/r/a/.team", &["x"], "T0")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.contains(&"teams.json".to_string()));
        assert!(
            !leftovers.iter().any(|n| n.ends_with(".tmp")),
            "no temp file should survive a successful rename: {leftovers:?}"
        );
    }

    #[test]
    fn load_corrupt_file_degrades_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(dir.path().join(FILE), b"{ this is not json").unwrap();
        let reg = load(dir.path()).unwrap();
        assert!(
            reg.teams.is_empty(),
            "corrupt store reads as empty, not an error"
        );
    }

    #[test]
    fn is_orphan_tracks_presence_of_compose() {
        let live = entry("main", "/r/live/.team", &["x"], "T0");
        let gone = entry("main", "/r/gone/.team", &["x"], "T0");
        // Modern shape: root IS the .team dir, compose directly inside it.
        let extant: HashSet<PathBuf> = [PathBuf::from("/r/live/.team/team-compose.yaml")].into();
        let exists = |p: &Path| extant.contains(p);
        assert!(!is_orphan(&live, &exists), "live root keeps its compose");
        assert!(is_orphan(&gone, &exists), "missing compose ⇒ orphan");
    }

    #[test]
    fn is_orphan_falls_back_to_bare_root_layout_for_sessions_parity() {
        // Defensive fallback only: `up` records the `.team` dir as `root`,
        // so this bare-project-dir shape isn't a path `up` writes — it's
        // here for parity with `sessions::root_has_compose`, which accepts
        // a root pointing at the project dir with the compose under .team/.
        let e = entry("main", "/r/proj", &["x"], "T0");
        let extant: HashSet<PathBuf> = [PathBuf::from("/r/proj/.team/team-compose.yaml")].into();
        let exists = |p: &Path| extant.contains(p);
        assert!(!is_orphan(&e, &exists));
    }

    fn rentry(project: &str, agent: &str, session: &str) -> RosterEntry {
        RosterEntry {
            project_id: project.into(),
            agent: agent.into(),
            tmux_session: session.into(),
        }
    }

    #[test]
    fn roster_for_root_rebuilds_sessions_from_recorded_prefix() {
        // entry() records tmux_prefix "t-"; the roster rebuilds each agent's
        // session as `{prefix}{project}-{agent}`, only for the asked root.
        let mut reg = Registry::default();
        reg.teams
            .push(entry("main", "/r/a/.team", &["compass", "scout"], "T0"));
        reg.teams.push(entry("ops", "/r/b/.team", &["otto"], "T0"));
        assert_eq!(
            reg.roster_for_root(Path::new("/r/a/.team")),
            vec![
                rentry("main", "compass", "t-main-compass"),
                rentry("main", "scout", "t-main-scout"),
            ]
        );
    }

    #[test]
    fn reap_targets_drops_removed_keeps_current() {
        // Roster has compass+scout; the current compose (desired) keeps only
        // compass → scout is the orphan, compass is never reaped.
        let roster = vec![
            rentry("main", "compass", "t-main-compass"),
            rentry("main", "scout", "t-main-scout"),
        ];
        let desired: HashSet<String> = ["main:compass".to_string()].into_iter().collect();
        assert_eq!(
            reap_targets(&roster, &desired, None, false),
            vec![rentry("main", "scout", "t-main-scout")]
        );
    }

    #[test]
    fn reap_targets_skips_entirely_on_per_agent_teardown() {
        // scout would be an orphan (empty desired), but a `--agent` teardown
        // is partial → reap nothing.
        let roster = vec![rentry("main", "scout", "t-main-scout")];
        assert!(reap_targets(&roster, &HashSet::new(), None, true).is_empty());
    }

    #[test]
    fn reap_targets_honors_project_scope() {
        // Two projects share a root; a `--project main` reap touches only
        // main's orphans, never ops'.
        let roster = vec![
            rentry("main", "scout", "t-main-scout"),
            rentry("ops", "otto", "t-ops-otto"),
        ];
        assert_eq!(
            reap_targets(&roster, &HashSet::new(), Some("main"), false),
            vec![rentry("main", "scout", "t-main-scout")]
        );
    }

    #[test]
    fn orphans_for_root_loads_and_diffs_against_desired() {
        // End-to-end over a real on-disk store (what `down`/`reload` reap
        // through): a team recorded up at this root with compass+scout; the
        // current compose keeps only compass → scout is the orphan. compass
        // (still desired) is never returned — no false positive.
        let dir = tempfile::tempdir().unwrap();
        upsert(
            dir.path(),
            entry("main", "/r/a/.team", &["compass", "scout"], "T0"),
        )
        .unwrap();
        let desired: HashSet<String> = ["main:compass".to_string()].into_iter().collect();
        assert_eq!(
            orphans_for_root(dir.path(), Path::new("/r/a/.team"), &desired, None, false).unwrap(),
            vec![rentry("main", "scout", "t-main-scout")]
        );

        // A missing store loads as empty → no orphans, not an error: the
        // applied.json-absent reload path stays safe even with no registry.
        let empty = tempfile::tempdir().unwrap();
        assert!(
            orphans_for_root(empty.path(), Path::new("/r/a/.team"), &desired, None, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn same_name_other_root_flags_only_a_live_different_folder() {
        // `main` is registered up at /r/a/.team.
        let mut reg = Registry::default();
        reg.teams.push(entry("main", "/r/a/.team", &["x"], "T0"));
        let a_live = |p: &Path| p == Path::new("/r/a/.team/team-compose.yaml");

        // Launching `main` from a DIFFERENT folder, while /r/a is still live →
        // the conflict, naming the other root.
        assert_eq!(
            same_name_other_root(&reg, "main", Path::new("/r/b/.team"), &a_live),
            Some(PathBuf::from("/r/a/.team"))
        );
        // Re-up of the SAME (project, root) → not a conflict.
        assert_eq!(
            same_name_other_root(&reg, "main", Path::new("/r/a/.team"), &a_live),
            None
        );
        // A DIFFERENT project id at another folder → not a conflict.
        assert_eq!(
            same_name_other_root(&reg, "ops", Path::new("/r/b/.team"), &a_live),
            None
        );
        // The other folder is gone (orphan, no compose) → stale, don't block.
        let none_live = |_: &Path| false;
        assert_eq!(
            same_name_other_root(&reg, "main", Path::new("/r/b/.team"), &none_live),
            None
        );
    }
}
