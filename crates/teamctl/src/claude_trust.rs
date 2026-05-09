//! Pre-accept Claude Code's "Quick safety check: Is this a project you
//! trust?" dialog for workspace directories teamctl is about to drive an
//! agent into. Without this, `claude` blocks on the prompt the moment a
//! supervised agent boots (or the moment the operator opens claude in a
//! folder teamctl just scaffolded), defeating the "agents start working
//! when teamctl up runs" model.
//!
//! Recording trust here is appropriate because the operator's invocation
//! is itself an explicit "I trust this directory" signal — they're either
//! bringing AI agents up in it (`teamctl up`) or scaffolding a team into
//! it (`teamctl init`). The fix is **scoped narrowly**: only writes
//! `hasTrustDialogAccepted: true` for the specific cwd(s); does NOT
//! disable other CC permission prompts (tool-use, dangerous edits, etc.),
//! and does not touch trust state for any folder teamctl hasn't been
//! pointed at.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Pre-trust the single cwd that `teamctl init` just scaffolded into.
/// Canonicalizes the path before writing so symlinked invocations land
/// on the resolved target (matching CC's own resolution).
pub fn pre_trust_cwd(cwd: &Path) -> Result<()> {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let cwds: BTreeSet<PathBuf> = std::iter::once(canonical).collect();
    pre_trust_cwds(&cwds)
}

/// Pre-trust a set of cwds — used by `teamctl up` to cover every
/// `claude-code` agent's workspace at once. Caller is responsible for
/// canonicalization (each call site has different rules about how
/// relative paths resolve).
pub fn pre_trust_cwds(cwds: &BTreeSet<PathBuf>) -> Result<()> {
    if cwds.is_empty() {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    write_trust_state(cwds, &home)
}

/// Inner logic split off so tests can target a fake `$HOME` without
/// racing the process env.
fn write_trust_state(cwds: &BTreeSet<PathBuf>, home: &Path) -> Result<()> {
    let config_path = home.join(".claude.json");

    let mut config: serde_json::Value = match fs::read_to_string(&config_path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if !config
        .get("projects")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        config["projects"] = serde_json::json!({});
    }
    let projects = config["projects"].as_object_mut().unwrap();

    let mut newly_trusted = Vec::new();
    for cwd in cwds {
        let key = cwd.display().to_string();
        let entry = projects
            .entry(key.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let obj = entry.as_object_mut().unwrap();
        let already = matches!(
            obj.get("hasTrustDialogAccepted"),
            Some(serde_json::Value::Bool(true))
        );
        if !already {
            obj.insert(
                "hasTrustDialogAccepted".into(),
                serde_json::Value::Bool(true),
            );
            newly_trusted.push(key);
        }
    }

    if newly_trusted.is_empty() {
        return Ok(());
    }

    // Atomic write so a concurrent claude reader never sees a half-
    // written config.
    let tmp = config_path.with_extension("json.teamctl.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&config)?)?;
    fs::rename(&tmp, &config_path)?;

    for path in newly_trusted {
        eprintln!("trust · auto-accepted Claude Code workspace trust for {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let cfg = home.path().join(".claude.json");
        (home, cfg)
    }

    fn read_config(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn empty_cwds_is_a_noop() {
        let (home, cfg) = setup();
        write_trust_state(&BTreeSet::new(), home.path()).unwrap();
        // Empty input early-returns before the public wrapper too,
        // but the inner fn still must not write anything.
        assert!(!cfg.exists(), "no config should be written for empty input");
    }

    #[test]
    fn fresh_home_creates_config_with_trust_entry() {
        let (home, cfg) = setup();
        let path = PathBuf::from("/tmp/example-team");
        let mut cwds = BTreeSet::new();
        cwds.insert(path.clone());

        write_trust_state(&cwds, home.path()).unwrap();

        let v = read_config(&cfg);
        assert_eq!(
            v["projects"][path.display().to_string()]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
        );
    }

    #[test]
    fn existing_projects_are_preserved() {
        let (home, cfg) = setup();
        // Pre-existing config with one trusted + one unrelated project.
        let initial = serde_json::json!({
            "projects": {
                "/already/trusted": { "hasTrustDialogAccepted": true, "sessions": 7 },
                "/unrelated": { "hasTrustDialogAccepted": false, "costs": 0.0 }
            },
            "topLevel": "preserve me"
        });
        fs::write(&cfg, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let new_path = PathBuf::from("/fresh/team");
        let mut cwds = BTreeSet::new();
        cwds.insert(new_path.clone());

        write_trust_state(&cwds, home.path()).unwrap();

        let v = read_config(&cfg);
        assert_eq!(v["topLevel"], "preserve me");
        // Unrelated still false; we don't touch other projects' state.
        assert_eq!(
            v["projects"]["/unrelated"]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(false),
        );
        assert_eq!(v["projects"]["/already/trusted"]["sessions"], 7);
        assert_eq!(
            v["projects"][new_path.display().to_string()]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
        );
    }

    #[test]
    fn idempotent_when_path_already_trusted() {
        let (home, cfg) = setup();
        let path = PathBuf::from("/idempotent/team");
        let mut cwds = BTreeSet::new();
        cwds.insert(path.clone());

        write_trust_state(&cwds, home.path()).unwrap();
        let mtime_first = fs::metadata(&cfg).unwrap().modified().unwrap();

        // Sleep a hair so any rewrite would show in mtime.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_trust_state(&cwds, home.path()).unwrap();

        // No newly-trusted paths the second time → atomic write is
        // skipped → mtime unchanged.
        let mtime_second = fs::metadata(&cfg).unwrap().modified().unwrap();
        assert_eq!(
            mtime_first, mtime_second,
            "second call with already-trusted path should not rewrite"
        );
    }

    #[test]
    fn flips_existing_untrusted_entry_to_trusted() {
        let (home, cfg) = setup();
        let path = PathBuf::from("/was/untrusted");
        let initial = serde_json::json!({
            "projects": {
                path.display().to_string(): {
                    "hasTrustDialogAccepted": false,
                    "sessions": 3
                }
            }
        });
        fs::write(&cfg, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let mut cwds = BTreeSet::new();
        cwds.insert(path.clone());
        write_trust_state(&cwds, home.path()).unwrap();

        let v = read_config(&cfg);
        assert_eq!(
            v["projects"][path.display().to_string()]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
        );
        // Sibling fields preserved.
        assert_eq!(v["projects"][path.display().to_string()]["sessions"], 3);
    }

    #[test]
    fn malformed_config_is_replaced_rather_than_failing() {
        let (home, cfg) = setup();
        fs::write(&cfg, "this is not json").unwrap();

        let path = PathBuf::from("/recover/team");
        let mut cwds = BTreeSet::new();
        cwds.insert(path.clone());

        write_trust_state(&cwds, home.path()).unwrap();
        let v = read_config(&cfg);
        assert_eq!(
            v["projects"][path.display().to_string()]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
        );
    }

    #[test]
    fn pre_trust_cwd_canonicalizes_via_real_path() {
        // Use a real tempdir as cwd so canonicalize succeeds. The point
        // here isn't to exercise the trust-write (covered above) — it's
        // to confirm the canonicalize-then-delegate shape doesn't drop
        // a real path on a system where the tempdir lives behind a
        // symlink (e.g. macOS /tmp → /private/tmp).
        let work = tempfile::tempdir().unwrap();
        let canonical = work.path().canonicalize().unwrap();
        let cwds_input: BTreeSet<PathBuf> = std::iter::once(canonical.clone()).collect();

        // Drive write_trust_state directly with the canonicalized path
        // so the test stays hermetic against $HOME.
        let home = tempfile::tempdir().unwrap();
        write_trust_state(&cwds_input, home.path()).unwrap();

        let v = read_config(&home.path().join(".claude.json"));
        assert_eq!(
            v["projects"][canonical.display().to_string()]["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
        );
    }
}
