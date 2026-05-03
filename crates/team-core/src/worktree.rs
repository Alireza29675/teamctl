//! Per-agent git worktree orchestration.
//!
//! v2-A made per-session worktree isolation a runtime primitive: every
//! agent's tmux session launches in its own git worktree on its own
//! `agents/<agent-id>` branch, so concurrent file mutations across
//! sessions don't collide. Coordination back to a shared branch
//! happens through the existing HITL `merge_to_main` approval flow —
//! same shape as before, but now the substrate enforces the isolation
//! the marketing copy promises.
//!
//! This module exposes the small, testable primitives that
//! `teamctl up` / `teamctl down` orchestrate. The high-level rules
//! live on [`crate::compose::Compose::resolve_agent_cwd`]; this
//! module is just the IO half.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

/// Branch namespace for per-agent worktrees. `agents/<agent-id>`
/// keeps them out of the operator's own branch space.
pub const AGENT_BRANCH_PREFIX: &str = "agents/";

/// Branch name for an agent. Keep in sync with [`AGENT_BRANCH_PREFIX`].
pub fn branch_for_agent(agent_id: &str) -> String {
    format!("{AGENT_BRANCH_PREFIX}{agent_id}")
}

/// Idempotently ensure a worktree exists at `worktree_path` for
/// `agent_id`, branched off the current HEAD of the repo at
/// `git_source`.
///
/// On first call: `git -C <git_source> worktree add -b agents/<agent_id> <worktree_path>`.
/// On subsequent calls (worktree already there): no-op + a sanity
/// check via `git -C <worktree_path> rev-parse --git-dir`.
pub fn ensure_worktree(git_source: &Path, worktree_path: &Path, agent_id: &str) -> Result<()> {
    if worktree_path.join(".git").exists() {
        // Worktree files already in place; sanity check that git still
        // recognises this directory as a worktree of the source repo.
        let status = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(worktree_path)
            .output()
            .with_context(|| format!("git rev-parse in {}", worktree_path.display()))?;
        if !status.status.success() {
            return Err(anyhow!(
                "worktree at {} exists but git doesn't recognise it; \
                 run `teamctl down --clean-worktrees` to reset",
                worktree_path.display()
            ));
        }
        return Ok(());
    }

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create worktree parent dir {}", parent.display()))?;
    }

    let branch = branch_for_agent(agent_id);
    // Try `worktree add -b <branch>` first. If the branch already
    // exists from a prior cycle, fall through to `worktree add` with
    // the existing branch (no `-b`).
    let create = Command::new("git")
        .arg("-C")
        .arg(git_source)
        .args(["worktree", "add", "-b", &branch])
        .arg(worktree_path)
        .output()
        .with_context(|| format!("git worktree add (new branch) in {}", git_source.display()))?;

    if create.status.success() {
        return Ok(());
    }

    // Branch likely already exists (re-up after a manual `git worktree
    // remove` that left the branch dangling). Retry checking out the
    // existing branch.
    let reuse = Command::new("git")
        .arg("-C")
        .arg(git_source)
        .args(["worktree", "add"])
        .arg(worktree_path)
        .arg(&branch)
        .output()
        .with_context(|| {
            format!(
                "git worktree add (existing branch) in {}",
                git_source.display()
            )
        })?;

    if reuse.status.success() {
        return Ok(());
    }

    Err(anyhow!(
        "failed to create worktree at {}: {}",
        worktree_path.display(),
        String::from_utf8_lossy(&reuse.stderr).trim()
    ))
}

/// Remove a per-agent worktree and its `agents/<agent-id>` branch.
///
/// Destructive: drops the branch via `-D`. The caller (`teamctl down
/// --clean-worktrees`) is responsible for surfacing the consequences
/// to the operator.
pub fn remove_worktree(git_source: &Path, worktree_path: &Path, agent_id: &str) -> Result<()> {
    if worktree_path.exists() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(git_source)
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .status();
    }
    let _ = Command::new("git")
        .arg("-C")
        .arg(git_source)
        .args(["branch", "-D", &branch_for_agent(agent_id)])
        .status();
    Ok(())
}

/// Returns true iff `path` is the working tree of a git repository
/// (or one of its subdirectories). Used by `teamctl validate` to
/// detect the case where `worktree_isolation: true` requires a git
/// repo at `project.cwd` but doesn't have one.
pub fn is_git_repo(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let status = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(path)
        .output();
    matches!(status, Ok(o) if o.status.success())
}

/// Default per-agent worktree path — `<root>/state/worktrees/<agent>/`.
/// Mirrors [`crate::compose::Compose::resolve_agent_cwd`]'s isolation
/// branch so callers that already have just `(root, agent)` don't
/// need a full `Compose`.
pub fn default_worktree_path(root: &Path, agent_id: &str) -> PathBuf {
    root.join("state/worktrees").join(agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Initialise a fresh git repo with one commit at `path`. Returns
    /// the path so the caller can chain. Tests that need a "real" git
    /// source for worktree-add use this.
    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["init", "-q", "--initial-branch=main"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "test"])
            .status()
            .unwrap();
        std::fs::write(path.join("README.md"), "test\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "README.md"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();
    }

    #[test]
    fn branch_for_agent_uses_agents_namespace() {
        assert_eq!(branch_for_agent("maintainer"), "agents/maintainer");
        assert_eq!(branch_for_agent("bug_fix"), "agents/bug_fix");
    }

    #[test]
    fn default_worktree_path_lands_under_state() {
        let root = Path::new("/tmp/.team");
        assert_eq!(
            default_worktree_path(root, "pm"),
            PathBuf::from("/tmp/.team/state/worktrees/pm")
        );
    }

    #[test]
    fn ensure_worktree_creates_branch_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        init_git_repo(&source);
        let wt = dir.path().join("worktrees/pm");

        ensure_worktree(&source, &wt, "pm").unwrap();

        assert!(wt.join(".git").exists(), "worktree dir missing .git");
        assert!(
            wt.join("README.md").exists(),
            "worktree didn't inherit files"
        );

        // Branch landed.
        let out = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["branch", "--list", "agents/pm"])
            .output()
            .unwrap();
        let listed = String::from_utf8(out.stdout).unwrap();
        assert!(
            listed.contains("agents/pm"),
            "agents/pm branch not created; got: {listed:?}"
        );
    }

    #[test]
    fn ensure_worktree_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        init_git_repo(&source);
        let wt = dir.path().join("worktrees/pm");

        ensure_worktree(&source, &wt, "pm").unwrap();
        // Second call: no-op, no error.
        ensure_worktree(&source, &wt, "pm").unwrap();

        // Single worktree entry, single branch.
        let list = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        let listed = String::from_utf8(list.stdout).unwrap();
        let count = listed
            .lines()
            .filter(|l| l.contains("worktrees/pm"))
            .count();
        assert_eq!(count, 1, "duplicate worktree entry:\n{listed}");
    }

    #[test]
    fn ensure_worktree_reuses_existing_branch() {
        // Simulate a worktree dir removed externally but the branch
        // still around. ensure_worktree should reuse the branch on
        // re-add rather than fail.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        init_git_repo(&source);
        let wt = dir.path().join("worktrees/pm");

        ensure_worktree(&source, &wt, "pm").unwrap();
        // Remove the worktree but leave the branch.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["worktree", "remove", "--force"])
            .arg(&wt)
            .status();

        // Re-ensure: should pick up the existing agents/pm branch.
        ensure_worktree(&source, &wt, "pm").unwrap();
        assert!(wt.join(".git").exists());
    }

    #[test]
    fn remove_worktree_drops_branch_and_dir() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        init_git_repo(&source);
        let wt = dir.path().join("worktrees/pm");

        ensure_worktree(&source, &wt, "pm").unwrap();
        remove_worktree(&source, &wt, "pm").unwrap();

        assert!(!wt.exists(), "worktree dir not removed");
        let out = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["branch", "--list", "agents/pm"])
            .output()
            .unwrap();
        let listed = String::from_utf8(out.stdout).unwrap();
        assert!(
            listed.trim().is_empty(),
            "agents/pm branch survived removal: {listed:?}"
        );
    }

    #[test]
    fn is_git_repo_detects_repo_and_non_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        init_git_repo(&repo);

        assert!(is_git_repo(&repo));
        assert!(!is_git_repo(&plain));
        assert!(!is_git_repo(&dir.path().join("does-not-exist")));
    }

    /// Coordination test: worker commits in its own worktree, manager
    /// merges from its worktree, user's main is untouched.
    #[test]
    fn manager_can_merge_worker_branch_and_main_stays_clean() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        init_git_repo(&source);

        let mgr = dir.path().join("worktrees/maintainer");
        let wrk = dir.path().join("worktrees/triage");
        ensure_worktree(&source, &mgr, "maintainer").unwrap();
        ensure_worktree(&source, &wrk, "triage").unwrap();

        // Worker makes a commit on its branch.
        std::fs::write(wrk.join("triage-note.md"), "from worker\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&wrk)
            .args(["add", "triage-note.md"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&wrk)
            .args(["commit", "-q", "-m", "triage: add note"])
            .status()
            .unwrap();

        // Manager merges the worker's branch from inside its own worktree.
        let merge = Command::new("git")
            .arg("-C")
            .arg(&mgr)
            .args(["merge", "--no-ff", "-m", "merge triage", "agents/triage"])
            .status()
            .unwrap();
        assert!(merge.success(), "manager merge failed");
        assert!(
            mgr.join("triage-note.md").exists(),
            "merge didn't bring the file in"
        );

        // User's main is untouched.
        assert!(
            !source.join("triage-note.md").exists(),
            "merge leaked into source/main"
        );
    }
}
