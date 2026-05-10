//! `teamctl sessions` — list every teamctl-managed tmux session on this
//! host, across projects.
//!
//! Sessions started by `TmuxSupervisor::up` carry session-level tmux
//! user-options that uniquely identify them: `@teamctl 1` plus
//! `@teamctl-project`, `@teamctl-agent`, `@teamctl-root`. The command
//! reads those via `tmux show-options -v -t <session>` for an
//! unambiguous tuple.
//!
//! Sessions created by older builds (before the tagging convention
//! landed) are not surfaced. The historical name-shape
//! `<tmux_prefix><project>-<agent>` was considered as a fallback, but
//! `tmux_prefix` is per-project (configurable in compose) and there is
//! no host-wide registry of in-use prefixes. Restart agents via
//! `teamctl up` to pick up the tags and appear in this listing.
//!
//! Orphan = the session declares a `@teamctl-root` whose
//! `team-compose.yaml` is no longer present on disk (project moved or
//! deleted). The check accepts both root layouts: the modern one where
//! `compose.root` is the `.team/` directory itself, and the bare-root
//! fallback (operators may pass `--root <project-dir>` and tagging
//! would record that). A session is orphan only if neither candidate
//! resolves to a real file.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

/// One row in the rendered table / json array.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SessionRow {
    pub project: String,
    pub agent: String,
    pub session: String,
    pub started_unix: i64,
    pub root: Option<PathBuf>,
    /// `running` (session is up and root resolves) or `orphan` (root
    /// recorded on the session no longer holds a `team-compose.yaml`).
    pub status: String,
}

/// Fields parsed from one line of `tmux list-sessions -F '<fmt>'`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSession {
    name: String,
    created_unix: i64,
}

pub fn run(json: bool) -> Result<()> {
    let raw_listing = match tmux_list_sessions()? {
        Some(s) => s,
        None => {
            // No tmux server running → no sessions. Print empty output.
            if json {
                println!("[]");
            } else {
                print_table(&[]);
            }
            return Ok(());
        }
    };
    let raws = parse_list_sessions(&raw_listing);

    let opt_lookup = |session: &str, key: &str| tmux_session_option(session, key);
    let rows = classify(&raws, &opt_lookup, |p| p.exists());

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_table(&rows);
    }
    Ok(())
}

fn print_table(rows: &[SessionRow]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    println!(
        "{:<20} {:<20} {:<14} {:<10}",
        "PROJECT", "AGENT", "STARTED", "STATUS",
    );
    if rows.is_empty() {
        return;
    }
    for r in rows {
        let started = format_relative(now.saturating_sub(r.started_unix));
        println!(
            "{:<20} {:<20} {:<14} {:<10}",
            truncate(&r.project, 20),
            truncate(&r.agent, 20),
            started,
            r.status,
        );
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

/// Pure classifier. Takes raw session listings, an option-lookup
/// closure (`(session, key) -> Option<value>`), and a path-existence
/// predicate. Returns the rows we'd render. Sessions without the
/// `@teamctl=1` marker are skipped — there's no host-wide registry of
/// teamctl name-prefixes, so name-shape detection is unsafe.
///
/// Split out from `run` so unit tests can exercise the full
/// classification matrix without shelling out to tmux.
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
    rows.sort_by(|a, b| a.project.cmp(&b.project).then(a.agent.cmp(&b.agent)));
    rows
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

    #[test]
    fn parse_list_sessions_handles_normal_output() {
        let raw = "a-hello-manager|1700000000\nrandom|1700000050\n";
        let parsed = parse_list_sessions(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "a-hello-manager");
        assert_eq!(parsed[0].created_unix, 1700000000);
        assert_eq!(parsed[1].name, "random");
    }

    #[test]
    fn parse_list_sessions_skips_malformed_lines() {
        let raw = "no-pipe-here\na-good|1700000100\n|missing-name\n";
        let parsed = parse_list_sessions(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "a-good");
    }

    #[test]
    fn classify_tagged_session_reads_metadata() {
        let raws = vec![RawSession {
            name: "weird-session-name".into(),
            created_unix: 1700000000,
        }];
        let opts = opt_map(&[
            ("weird-session-name", "@teamctl", "1"),
            ("weird-session-name", "@teamctl-project", "newsroom"),
            ("weird-session-name", "@teamctl-agent", "editor-in-chief"),
            ("weird-session-name", "@teamctl-root", "/tmp/exists"),
        ]);
        let rows = classify(&raws, &opts, |_| true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].project, "newsroom");
        assert_eq!(rows[0].agent, "editor-in-chief");
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[0].root.as_deref(), Some(Path::new("/tmp/exists")));
    }

    #[test]
    fn classify_tagged_session_with_missing_root_is_orphan() {
        let raws = vec![RawSession {
            name: "a-foo-bar".into(),
            created_unix: 1700000000,
        }];
        let opts = opt_map(&[
            ("a-foo-bar", "@teamctl", "1"),
            ("a-foo-bar", "@teamctl-project", "foo"),
            ("a-foo-bar", "@teamctl-agent", "bar"),
            ("a-foo-bar", "@teamctl-root", "/tmp/gone"),
        ]);
        let rows = classify(&raws, &opts, |_| false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "orphan");
    }

    #[test]
    fn classify_running_when_root_is_dot_team_directory() {
        // Modern shape: `compose.root` IS the `.team/` directory, so
        // `<root>/team-compose.yaml` resolves directly. Must not be
        // reported as orphan.
        let raws = vec![RawSession {
            name: "t-news-editor".into(),
            created_unix: 1,
        }];
        let opts = opt_map(&[
            ("t-news-editor", "@teamctl", "1"),
            ("t-news-editor", "@teamctl-project", "news"),
            ("t-news-editor", "@teamctl-agent", "editor"),
            ("t-news-editor", "@teamctl-root", "/repos/news/.team"),
        ]);
        let extant: HashSet<PathBuf> =
            [PathBuf::from("/repos/news/.team/team-compose.yaml")].into();
        let rows = classify(&raws, &opts, |p| extant.contains(p));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "running");
    }

    #[test]
    fn classify_running_when_root_is_project_directory() {
        // Bare-root shape: `@teamctl-root` is the project root and
        // `team-compose.yaml` lives under `<root>/.team/`. Must also
        // resolve to running, not orphan.
        let raws = vec![RawSession {
            name: "t-news-editor".into(),
            created_unix: 1,
        }];
        let opts = opt_map(&[
            ("t-news-editor", "@teamctl", "1"),
            ("t-news-editor", "@teamctl-project", "news"),
            ("t-news-editor", "@teamctl-agent", "editor"),
            ("t-news-editor", "@teamctl-root", "/repos/news"),
        ]);
        let extant: HashSet<PathBuf> =
            [PathBuf::from("/repos/news/.team/team-compose.yaml")].into();
        let rows = classify(&raws, &opts, |p| extant.contains(p));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "running");
    }

    #[test]
    fn classify_skips_untagged_sessions_regardless_of_name() {
        // Without the `@teamctl=1` marker, name-shape detection is
        // unsafe (per-project tmux_prefix is not host-discoverable).
        // Pre-tag agent sessions, bot sessions, and unrelated tmux
        // sessions all fall through.
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
    fn classify_sorts_by_project_then_agent() {
        let raws = vec![
            RawSession {
                name: "s1".into(),
                created_unix: 1,
            },
            RawSession {
                name: "s2".into(),
                created_unix: 2,
            },
            RawSession {
                name: "s3".into(),
                created_unix: 3,
            },
        ];
        let opts = opt_map(&[
            ("s1", "@teamctl", "1"),
            ("s1", "@teamctl-project", "zeta"),
            ("s1", "@teamctl-agent", "bot"),
            ("s2", "@teamctl", "1"),
            ("s2", "@teamctl-project", "alpha"),
            ("s2", "@teamctl-agent", "zeta"),
            ("s3", "@teamctl", "1"),
            ("s3", "@teamctl-project", "alpha"),
            ("s3", "@teamctl-agent", "bot"),
        ]);
        let rows = classify(&raws, &opts, |_| true);
        let order: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.project.as_str(), r.agent.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![("alpha", "bot"), ("alpha", "zeta"), ("zeta", "bot")]
        );
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
