//! Once-per-day "update available" nudge surfaced on `teamctl status`
//! and `teamctl up` (T-129).
//!
//! Operators ran `teamctl update --check` rarely — the existing surface
//! required them to remember it existed. This module folds the version
//! probe into commands they already touch every shift, with three
//! invariants:
//!
//! 1. **Once per day per compose root.** A cache file at
//!    `<root>/state/update-check.json` records the last fetched
//!    `latest`, the binary `current` it was checked against, and the
//!    `checked_on` date. Same-day + same-current → skip the network.
//!    A binary upgrade naturally invalidates the cache because
//!    `current` flips. Note: the throttle is per-compose-root, not
//!    per-host — an operator running teamctl across N roots on one
//!    machine performs N daily fetches. Still well within GitHub's
//!    60/hour anonymous limit; moving to `~/.cache/teamctl/` would
//!    expand scope past this ticket.
//! 2. **Silent on every failure.** No network, API rate-limit, parse
//!    error, on-disk IO error, missing dir — none of these surface to
//!    the operator. The banner is a soft hint; failing it loud would
//!    defeat its low-friction shape and pollute scripts piping
//!    `teamctl status`.
//! 3. **Banner emits to stderr.** `teamctl status` is column-formatted
//!    and commonly piped (`| grep`, `| awk`); stdout must stay clean.
//!    Stderr also matches how `teamctl up` already routes warnings.
//!
//! The fetch path inherits `update::curl_get`'s 15s `--max-time`, so
//! on a slow or unreachable GitHub `teamctl status` can stall up to
//! 15s — once per day per root. Same constraint as
//! `teamctl update --check` today; not a regression here.

use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};

use super::update::{compare_versions, fetch_latest_version, VersionOrder, CURRENT_VERSION};

/// Cache file persisted under `<root>/state/`. Schema is internal —
/// the file is rewritten in place every time we hit the network and a
/// shape mismatch on read silently degrades to "no cache, fetch now."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CheckRecord {
    /// `YYYY-MM-DD` in **local** time. Once-per-day from a human's
    /// POV; matches the `rl_watch` convention elsewhere in this crate.
    checked_on: String,
    /// The binary version the cache was recorded for. Stamping this
    /// means a binary upgrade invalidates the cache automatically — a
    /// fresh `teamctl update` re-checks immediately rather than waiting
    /// for tomorrow.
    current: String,
    /// Latest version observed from the GitHub releases API at the
    /// time of the check.
    latest: String,
}

fn cache_path(root: &Path) -> PathBuf {
    root.join("state/update-check.json")
}

/// Top-level hook called from `teamctl status` and `teamctl up`. Never
/// errors — every failure path swallows.
pub fn maybe_print_banner(root: &Path) {
    let _ = print_banner_inner(root);
}

/// Inner happy path: returns `Some(())` when something was actually
/// printed, `None` when we either skipped due to a fail-quiet branch or
/// the local version was already up-to-date. Both are equally fine —
/// callers don't care.
fn print_banner_inner(root: &Path) -> Option<()> {
    let today = Local::now().date_naive();
    let prior = read_cache(&cache_path(root));
    let latest = match decide_check(prior.as_ref(), today, CURRENT_VERSION) {
        Decision::Skip(cached_latest) => cached_latest,
        Decision::Fetch => {
            let fetched = fetch_latest_version().ok()?;
            // Best-effort write — a failed write doesn't block the
            // banner from printing this run; tomorrow's invocation
            // will just hit the network again.
            let _ = write_cache(
                &cache_path(root),
                &CheckRecord {
                    checked_on: format_date(today),
                    current: CURRENT_VERSION.to_string(),
                    latest: fetched.clone(),
                },
            );
            fetched
        }
    };
    if matches!(
        compare_versions(CURRENT_VERSION, &latest),
        VersionOrder::Older
    ) {
        eprintln!("update available: {CURRENT_VERSION} → {latest} · run `teamctl update`");
        return Some(());
    }
    None
}

/// Whether to hit the network this invocation. Pulled out as a pure
/// function so the date-vs-cache logic is unit-testable without IO.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Cache is fresh for today AND was recorded against the same
    /// binary we're running. Reuse the cached `latest`.
    Skip(String),
    /// Cache is missing, stale, or recorded against a different
    /// binary. Refresh from GitHub.
    Fetch,
}

fn decide_check(prior: Option<&CheckRecord>, today: NaiveDate, current: &str) -> Decision {
    match prior {
        Some(rec) if rec.checked_on == format_date(today) && rec.current == current => {
            Decision::Skip(rec.latest.clone())
        }
        _ => Decision::Fetch,
    }
}

fn format_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn read_cache(path: &Path) -> Option<CheckRecord> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: &Path, rec: &CheckRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(rec)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn rec(checked_on: &str, current: &str, latest: &str) -> CheckRecord {
        CheckRecord {
            checked_on: checked_on.into(),
            current: current.into(),
            latest: latest.into(),
        }
    }

    #[test]
    fn decide_check_fetches_when_no_prior() {
        assert_eq!(
            decide_check(None, date(2026, 5, 10), "0.7.3"),
            Decision::Fetch,
        );
    }

    #[test]
    fn decide_check_skips_when_same_day_and_same_current() {
        let prior = rec("2026-05-10", "0.7.3", "0.7.4");
        assert_eq!(
            decide_check(Some(&prior), date(2026, 5, 10), "0.7.3"),
            Decision::Skip("0.7.4".into()),
        );
    }

    #[test]
    fn decide_check_fetches_on_different_day() {
        let prior = rec("2026-05-09", "0.7.3", "0.7.4");
        assert_eq!(
            decide_check(Some(&prior), date(2026, 5, 10), "0.7.3"),
            Decision::Fetch,
        );
    }

    #[test]
    fn decide_check_fetches_on_binary_upgrade_same_day() {
        // Operator ran `teamctl update` at noon, version flipped from
        // 0.7.3 to 0.7.4. The afternoon `teamctl status` should hit
        // the network rather than reuse the morning's cache, because
        // the cached `latest` may now equal the just-installed
        // `current` and the banner should disappear immediately.
        let prior = rec("2026-05-10", "0.7.3", "0.7.4");
        assert_eq!(
            decide_check(Some(&prior), date(2026, 5, 10), "0.7.4"),
            Decision::Fetch,
        );
    }

    #[test]
    fn cache_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/update-check.json");
        let original = rec("2026-05-10", "0.7.3", "0.7.4");
        write_cache(&path, &original).unwrap();
        let loaded = read_cache(&path).expect("cache should round-trip");
        assert_eq!(loaded, original);
    }

    #[test]
    fn read_cache_returns_none_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/update-check.json");
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn read_cache_returns_none_on_malformed_json() {
        // Tolerant: a future schema-shape change must silently fall
        // back to "fetch from GitHub" rather than crashing the banner
        // hook (which would defeat the silent-on-failure invariant).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/update-check.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn read_cache_returns_none_on_wrong_shape() {
        // Same defensive posture for "looks like JSON but isn't ours."
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/update-check.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"unrelated":"object"}"#).unwrap();
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn cache_path_lives_under_root_state_dir() {
        // Pin the on-disk location so a future change can't silently
        // move it (operators may grep for it, install scripts may
        // rotate it, etc.).
        let p = cache_path(Path::new("/teamctl"));
        assert_eq!(p, Path::new("/teamctl/state/update-check.json"));
    }
}
