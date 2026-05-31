//! Bakes a build version into `TEAMCTL_BUILD_VERSION` so dev/local builds
//! self-identify (`0.8.6-53-g5aba5f3`, `-dirty` when the tree has uncommitted
//! changes) while tagged releases stay clean (`0.8.7`) and builds without git
//! (extracted tarball, crates.io) fall back to the plain `CARGO_PKG_VERSION`.
//! The build must never fail because git is absent. (#359)

use std::process::Command;

fn main() {
    // Refresh the baked version when HEAD or refs move. Resolved via git so
    // the paths are correct in linked worktrees too; best-effort and skipped
    // when git is unavailable (the value then tracks the last clean build).
    for rel in ["HEAD", "refs"] {
        if let Some(path) = git_path(rel) {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let version = describe().unwrap_or_else(cargo_version);
    println!("cargo:rustc-env=TEAMCTL_BUILD_VERSION={version}");
}

/// `git describe --tags --always --dirty`, with the leading `v` stripped so
/// dev (`0.8.6-53-g5aba5f3`) and release (`0.8.6`) formats match
/// `CARGO_PKG_VERSION`. Returns `None` — so the caller falls back to
/// `CARGO_PKG_VERSION` — on any git failure (missing binary, not a repo) and
/// when the result isn't a version: a tag-less repo's bare `--always` commit
/// hash has no `.`, so we route it to the fallback per the issue's no-tags
/// intent (a clean `0.8.6` beats a bare hash in `--version`).
fn describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let trimmed = raw.trim();
    let version = trimmed.strip_prefix('v').unwrap_or(trimmed).to_string();
    // A real version-describe always carries a `.` from the semver tag; a bare
    // commit hash does not. Tags here are dotted semver (`v0.8.6`).
    version.contains('.').then_some(version)
}

/// Absolute-or-relative path to a git internal file (`HEAD`, `refs`), resolved
/// through git so it's correct inside linked worktrees. `None` when git is
/// unavailable — the rerun hint is then simply omitted.
fn git_path(rel: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", rel])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// The plain Cargo manifest version. Cargo always sets `CARGO_PKG_VERSION` for
/// build scripts; the default is belt-and-suspenders so the build never fails.
fn cargo_version() -> String {
    std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string())
}
