//! `teamctl sessions` — list every teamctl-managed tmux session on this
//! host, aggregated per project. Plus `teamctl sessions kill <project>`
//! to tear down every session belonging to one project (the only path
//! that works for orphans, where the compose file is gone and
//! `teamctl down` can't reach the project).
//!
//! Sessions started by `TmuxSupervisor::up` carry session-level tmux
//! user-options that uniquely identify them: `@teamctl 1` plus
//! `@teamctl-project`, `@teamctl-agent`, `@teamctl-root`. The command
//! reads those via `tmux show-options -v -t <session>`. Sessions
//! created by older builds (before the tagging convention landed) are
//! not surfaced — `tmux_prefix` is per-project and there is no
//! host-wide registry of in-use prefixes, so name-shape detection is
//! unsafe. Restart agents via `teamctl up` to pick up the tags.
//!
//! Orphan = a project whose recorded `@teamctl-root` no longer holds a
//! `team-compose.yaml` (under either `<root>/team-compose.yaml` or
//! `<root>/.team/team-compose.yaml`). When any project is orphan, the
//! human listing prints a footer pointing at the kill subcommand.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

/// Aggregated row — one per project. The human listing and `--json`
/// both emit this shape (rather than per-session) because the operator
/// view is "where can I cd to manage this team."
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ProjectRow {
    pub project: String,
    pub cwd: Option<PathBuf>,
    /// Sorted agent names (one per running session for the project).
    pub agents: Vec<String>,
    /// Oldest session in the project, unix epoch seconds.
    pub started_unix: i64,
    /// `running` (every session resolves to a live compose root) or
    /// `orphan` (any session's recorded root no longer holds a
    /// `team-compose.yaml`).
    pub status: String,
}

/// Internal per-session row, before aggregation.
#[derive(Debug, PartialEq, Eq)]
struct SessionRow {
    project: String,
    agent: String,
    session: String,
    started_unix: i64,
    root: Option<PathBuf>,
    status: String,
}

/// Fields parsed from one line of `tmux list-sessions -F '<fmt>'`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSession {
    name: String,
    created_unix: i64,
}

pub fn run(json: bool) -> Result<()> {
    let projects = collect_projects()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
    } else {
        print_table(&projects);
    }
    Ok(())
}

/// Kill every teamctl-managed tmux session whose `@teamctl-project`
/// equals `project`. The orphan case — where the compose file is gone
/// — is the whole reason this exists; for live projects an operator
/// can equally well `cd` and run `teamctl down`. No confirmation
/// prompt: kills are reversible via `teamctl up`.
pub fn kill(project: &str) -> Result<()> {
    let raw_listing = tmux_list_sessions()?.unwrap_or_default();
    let raws = parse_list_sessions(&raw_listing);
    let opt_lookup = |session: &str, key: &str| tmux_session_option(session, key);

    let mut killed: Vec<String> = Vec::new();
    for r in &raws {
        if opt_lookup(&r.name, "@teamctl").as_deref() != Some("1") {
            continue;
        }
        if opt_lookup(&r.name, "@teamctl-project").as_deref() != Some(project) {
            continue;
        }
        let status = Command::new("tmux")
            .args(["kill-session", "-t", &r.name])
            .status()
            .with_context(|| format!("spawn tmux kill-session for {}", r.name))?;
        if status.success() {
            killed.push(r.name.clone());
        } else {
            eprintln!("warning: tmux kill-session {} exited {status}", r.name);
        }
    }

    if killed.is_empty() {
        println!("no teamctl-managed sessions found for project `{project}`");
    } else {
        println!(
            "killed {} session(s) for `{project}`: {}",
            killed.len(),
            killed.join(", ")
        );
    }
    Ok(())
}

fn collect_projects() -> Result<Vec<ProjectRow>> {
    let raw_listing = match tmux_list_sessions()? {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };
    let raws = parse_list_sessions(&raw_listing);
    let opt_lookup = |session: &str, key: &str| tmux_session_option(session, key);
    let session_rows = classify(&raws, &opt_lookup, |p| p.exists());
    Ok(aggregate(session_rows))
}

fn print_table(rows: &[ProjectRow]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    println!(
        "{:<20} {:<50} {:<14} {:<8}",
        "PROJECT", "CWD", "STARTED", "STATUS",
    );
    if rows.is_empty() {
        return;
    }
    for r in rows {
        let started = format_relative(now.saturating_sub(r.started_unix));
        let cwd_display = r
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<20} {:<50} {:<14} {:<8}",
            truncate(&r.project, 20),
            truncate(&cwd_display, 50),
            started,
            r.status,
        );
    }
    if rows.iter().any(|r| r.status == "orphan") {
        println!();
        println!("to kill an orphan team's sessions: teamctl sessions kill <PROJECT>");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.into()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn tmux_list_sessions() -> Result<Option<String>> {
    let out = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}|#{session_created}"])
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("spawn tmux list-sessions"),
    };
    if !out.status.success() {
        // tmux exits non-zero with "no server running" when nothing is up;
        // treat that as zero sessions rather than an error.
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// `true` if any teamctl-managed tmux session (tagged `@teamctl=1`) is live
/// anywhere on this host. Host-wide refcount for the caffeinate keep-awake
/// lifecycle (T-370): the assertion lives while ANY team is up and is released
/// only when the last one goes down. Reuses the same tagging + listing the
/// `sessions` command relies on, so it sees exactly the teamctl-managed
/// sessions and ignores unrelated tmux.
///
/// Gated to macOS because its only caller (`caffeinate::stop_if_last`) is
/// macOS-only; in a binary crate `pub` does not suppress dead-code, so leaving
/// it ungated trips `-D warnings` on non-macOS CI.
#[cfg(target_os = "macos")]
pub fn any_teamctl_session_running() -> bool {
    let raw = match tmux_list_sessions() {
        Ok(Some(s)) => s,
        _ => return false,
    };
    parse_list_sessions(&raw)
        .iter()
        .any(|r| tmux_session_option(&r.name, "@teamctl").as_deref() == Some("1"))
}

fn tmux_session_option(session: &str, key: &str) -> Option<String> {
    let out = Command::new("tmux")
        .args(["show-options", "-v", "-t", session, key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_list_sessions(raw: &str) -> Vec<RawSession> {
    raw.lines()
        .filter_map(|line| {
            let (name, created) = line.split_once('|')?;
            let created_unix = created.trim().parse().ok()?;
            Some(RawSession {
                name: name.trim().to_string(),
                created_unix,
            })
        })
        .collect()
}

/// `true` if the recorded root still holds a `team-compose.yaml`,
/// looked up under either layout: `<root>/team-compose.yaml` (the
/// modern shape — `compose.root` IS the `.team/` directory) or
/// `<root>/.team/team-compose.yaml` (operator stored the project root
/// itself). Returns `false` only when neither candidate exists.
fn root_has_compose(root: &Path, path_exists: &impl Fn(&Path) -> bool) -> bool {
    path_exists(&root.join("team-compose.yaml"))
        || path_exists(&root.join(".team").join("team-compose.yaml"))
}

/// Pure classifier. Sessions without the `@teamctl=1` marker are
/// skipped — there's no host-wide registry of teamctl name-prefixes,
/// so name-shape detection is unsafe. Returns per-session rows;
/// `aggregate` collapses them into per-project rows.
fn classify(
    raws: &[RawSession],
    options: &dyn Fn(&str, &str) -> Option<String>,
    path_exists: impl Fn(&Path) -> bool,
) -> Vec<SessionRow> {
    let mut rows = Vec::new();
    for r in raws {
        if options(&r.name, "@teamctl").as_deref() != Some("1") {
            continue;
        }
        let project = options(&r.name, "@teamctl-project").unwrap_or_default();
        let agent = options(&r.name, "@teamctl-agent").unwrap_or_default();
        let root = options(&r.name, "@teamctl-root").map(PathBuf::from);
        let status = match &root {
            Some(p) if !root_has_compose(p, &path_exists) => "orphan",
            _ => "running",
        };
        rows.push(SessionRow {
            project,
            agent,
            session: r.name.clone(),
            started_unix: r.created_unix,
            root,
            status: status.into(),
        });
    }
    rows
}

/// Collapse per-session rows into per-project rows. Status is the
/// worst-of (orphan trumps running). Started is the oldest session.
/// CWD is the first non-`None` root encountered for the project; in
/// practice every session for a project shares the same root.
fn aggregate(sessions: Vec<SessionRow>) -> Vec<ProjectRow> {
    use std::collections::BTreeMap;
    let mut by_project: BTreeMap<String, Vec<SessionRow>> = BTreeMap::new();
    for s in sessions {
        by_project.entry(s.project.clone()).or_default().push(s);
    }
    let mut out = Vec::with_capacity(by_project.len());
    for (project, mut group) in by_project {
        group.sort_by(|a, b| a.agent.cmp(&b.agent));
        let agents: Vec<String> = group.iter().map(|s| s.agent.clone()).collect();
        let cwd = group.iter().find_map(|s| s.root.clone());
        let started_unix = group
            .iter()
            .map(|s| s.started_unix)
            .min()
            .unwrap_or_default();
        let status = if group.iter().any(|s| s.status == "orphan") {
            "orphan"
        } else {
            "running"
        };
        out.push(ProjectRow {
            project,
            cwd,
            agents,
            started_unix,
            status: status.into(),
        });
    }
    out
}

fn format_relative(seconds_ago: i64) -> String {
    if seconds_ago < 0 {
        return "just now".into();
    }
    let s = seconds_ago;
    if s < 60 {
        return format!("{s}s ago");
    }
    let m = s / 60;
    if m < 60 {
        let rem = s % 60;
        return if rem == 0 {
            format!("{m}m ago")
        } else {
            format!("{m}m {rem}s ago")
        };
    }
    let h = m / 60;
    if h < 24 {
        let rem = m % 60;
        return if rem == 0 {
            format!("{h}h ago")
        } else {
            format!("{h}h {rem}m ago")
        };
    }
    let d = h / 24;
    let rem = h % 24;
    if rem == 0 {
        format!("{d}d ago")
    } else {
        format!("{d}d {rem}h ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::collections::HashSet;

    fn opt_map(pairs: &[(&str, &str, &str)]) -> impl Fn(&str, &str) -> Option<String> {
        let mut m: HashMap<(String, String), String> = HashMap::new();
        for (s, k, v) in pairs {
            m.insert(((*s).into(), (*k).into()), (*v).into());
        }
        move |s, k| m.get(&(s.to_string(), k.to_string())).cloned()
    }

    fn tagged(
        session: &str,
        project: &str,
        agent: &str,
        root: &str,
    ) -> Vec<(String, String, String)> {
        vec![
            (session.into(), "@teamctl".into(), "1".into()),
            (session.into(), "@teamctl-project".into(), project.into()),
            (session.into(), "@teamctl-agent".into(), agent.into()),
            (session.into(), "@teamctl-root".into(), root.into()),
        ]
    }

    fn opts_from(triples: Vec<(String, String, String)>) -> impl Fn(&str, &str) -> Option<String> {
        let mut m: HashMap<(String, String), String> = HashMap::new();
        for (s, k, v) in triples {
            m.insert((s, k), v);
        }
        move |s, k| m.get(&(s.to_string(), k.to_string())).cloned()
    }

    #[test]
    fn parse_list_sessions_handles_normal_output() {
        let raw = "a-hello-manager|1700000000\nrandom|1700000050\n";
        let parsed = parse_list_sessions(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "a-hello-manager");
        assert_eq!(parsed[0].created_unix, 1700000000);
    }

    #[test]
    fn parse_list_sessions_skips_malformed_lines() {
        let raw = "no-pipe-here\na-good|1700000100\n|missing-name\n";
        let parsed = parse_list_sessions(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "a-good");
    }

    #[test]
    fn classify_skips_untagged_sessions_regardless_of_name() {
        // No `@teamctl=1` marker → skipped, even for names that look
        // teamctl-shaped (legacy agents, bot sessions, unrelated tmux).
        let raws = vec![
            RawSession {
                name: "t-teamctl-hugo".into(),
                created_unix: 1,
            },
            RawSession {
                name: "a-bot-teamctl-pm".into(),
                created_unix: 2,
            },
            RawSession {
                name: "vim".into(),
                created_unix: 3,
            },
        ];
        let opts = opt_map(&[]);
        assert!(classify(&raws, &opts, |_| true).is_empty());
    }

    #[test]
    fn aggregate_collapses_sessions_per_project_and_sorts_agents() {
        let raws = vec![
            RawSession {
                name: "s1".into(),
                created_unix: 200,
            },
            RawSession {
                name: "s2".into(),
                created_unix: 100,
            },
            RawSession {
                name: "s3".into(),
                created_unix: 300,
            },
        ];
        let mut triples = Vec::new();
        triples.extend(tagged("s1", "alpha", "zeta", "/r/alpha/.team"));
        triples.extend(tagged("s2", "alpha", "bot", "/r/alpha/.team"));
        triples.extend(tagged("s3", "beta", "x", "/r/beta/.team"));
        let opts = opts_from(triples);
        let extant: HashSet<PathBuf> = [
            PathBuf::from("/r/alpha/.team/team-compose.yaml"),
            PathBuf::from("/r/beta/.team/team-compose.yaml"),
        ]
        .into();
        let session_rows = classify(&raws, &opts, |p| extant.contains(p));
        let projects = aggregate(session_rows);
        assert_eq!(projects.len(), 2);
        // BTreeMap iteration is alpha-sorted by project name.
        assert_eq!(projects[0].project, "alpha");
        assert_eq!(projects[0].agents, vec!["bot", "zeta"]); // agent-sorted
        assert_eq!(projects[0].started_unix, 100); // oldest of s1+s2
        assert_eq!(projects[0].status, "running");
        assert_eq!(
            projects[0].cwd.as_deref(),
            Some(Path::new("/r/alpha/.team"))
        );
        assert_eq!(projects[1].project, "beta");
        assert_eq!(projects[1].agents, vec!["x"]);
    }

    #[test]
    fn aggregate_status_is_orphan_if_any_session_is_orphan() {
        let raws = vec![
            RawSession {
                name: "s1".into(),
                created_unix: 1,
            },
            RawSession {
                name: "s2".into(),
                created_unix: 2,
            },
        ];
        let mut triples = Vec::new();
        triples.extend(tagged("s1", "mix", "alive", "/r/exists/.team"));
        triples.extend(tagged("s2", "mix", "stale", "/r/gone/.team"));
        let opts = opts_from(triples);
        let extant: HashSet<PathBuf> = [PathBuf::from("/r/exists/.team/team-compose.yaml")].into();
        let projects = aggregate(classify(&raws, &opts, |p| extant.contains(p)));
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].status, "orphan");
    }

    #[test]
    fn classify_running_when_root_is_dot_team_directory() {
        // Modern shape: `compose.root` IS the `.team/` directory.
        let raws = vec![RawSession {
            name: "t-news-editor".into(),
            created_unix: 1,
        }];
        let opts = opts_from(tagged(
            "t-news-editor",
            "news",
            "editor",
            "/repos/news/.team",
        ));
        let extant: HashSet<PathBuf> =
            [PathBuf::from("/repos/news/.team/team-compose.yaml")].into();
        let rows = classify(&raws, &opts, |p| extant.contains(p));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "running");
    }

    #[test]
    fn classify_running_when_root_is_project_directory() {
        // Bare-root shape: `@teamctl-root` is the project root and
        // `team-compose.yaml` lives under `<root>/.team/`.
        let raws = vec![RawSession {
            name: "t-news-editor".into(),
            created_unix: 1,
        }];
        let opts = opts_from(tagged("t-news-editor", "news", "editor", "/repos/news"));
        let extant: HashSet<PathBuf> =
            [PathBuf::from("/repos/news/.team/team-compose.yaml")].into();
        let rows = classify(&raws, &opts, |p| extant.contains(p));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "running");
    }

    #[test]
    fn format_relative_covers_buckets() {
        assert_eq!(format_relative(-5), "just now");
        assert_eq!(format_relative(0), "0s ago");
        assert_eq!(format_relative(45), "45s ago");
        assert_eq!(format_relative(60), "1m ago");
        assert_eq!(format_relative(125), "2m 5s ago");
        assert_eq!(format_relative(3_600), "1h ago");
        assert_eq!(format_relative(3_661), "1h 1m ago");
        assert_eq!(format_relative(86_400), "1d ago");
        assert_eq!(format_relative(90_000), "1d 1h ago");
    }
}
