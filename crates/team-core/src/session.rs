//! T-118: deterministic Claude Code session id derivation.
//!
//! Every claude-code agent spawn passes `--session-id <uuid>` so the
//! conversation persists across `teamctl down`/`up` cycles, crash
//! recovery, and host reboots. The id is a UUIDv5 derived from a
//! baked-in namespace plus the canonical agent string
//! `teamctl:<project>:<agent>`.
//!
//! Self-healing by construction: if the session-file at that UUID is
//! deleted (manual cleanup, claude session-dir reset, etc.), the next
//! spawn passes the same UUID and claude creates a fresh session at
//! it. No operator action required.
//!
//! Stable across rename of *external* things (claude version, host
//! machine, tmux session) but evicts cleanly when the project or
//! agent name changes — that's the right semantics: a renamed agent
//! is a new agent.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// Frozen UUIDv5 namespace for teamctl session-id derivation.
/// Generated once on 2026-05-10 and pinned forever — changing this
/// would invalidate every previously-resumed session across every
/// dogfooding installation. Treat as a constant.
///
/// Provenance: `uuid::Uuid::new_v4()` on 2026-05-10 in this repo's
/// development environment, pasted as a literal.
pub const TEAMCTL_SESSION_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6d, 0xd6, 0xc8, 0xa3, 0x44, 0xb6, 0x4a, 0x18, 0x9b, 0x05, 0x91, 0xc1, 0xe2, 0x57, 0xfb, 0x3d,
]);

/// Compose the canonical name string for a given project/agent pair.
/// Used as the UUIDv5 input AND surfaced verbatim to claude via the
/// `-n <name>` flag so the operator sees the agent identity in
/// claude's session picker / prompt box.
pub fn session_name(project: &str, agent: &str) -> String {
    format!("teamctl:{project}:{agent}")
}

/// Derive the deterministic Claude Code session id for a given
/// project/agent pair. Same inputs always yield the same UUID;
/// different agent or project yields a different UUID.
pub fn derive_session_id(project: &str, agent: &str) -> Uuid {
    Uuid::new_v5(
        &TEAMCTL_SESSION_NAMESPACE,
        session_name(project, agent).as_bytes(),
    )
}

/// `~/.claude`, derived from `$HOME` — the same base the agent wrapper
/// probes (`$HOME/.claude/projects/*/<uuid>.jsonl`, agent-wrapper.sh:166).
/// `None` when `$HOME` is unset, so callers can warn-and-skip rather than
/// guess a path.
pub fn claude_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
}

/// T-352: move aside the on-disk Claude session JSONL for `(project, agent)`
/// so the wrapper's resume-probe misses on the next spawn and Claude opens a
/// brand-new conversation at the *same* deterministic UUID (re-running
/// `BOOTSTRAP_PROMPT`). The `--fresh` escape hatch from always-on resume
/// (T-118); durable on-disk files are never touched — only the session JSONL.
///
/// `claude_home` is `~/.claude` (injected so tests don't touch the real home).
/// Globs `projects/*/<uuid>.jsonl` exactly like the wrapper, because Claude's
/// cwd→project-dir slug is observed-not-documented; the UUIDv5 is globally
/// unique so at most one file ever matches.
///
/// The move is a single `rename(2)` to `<uuid>.jsonl.bak` within the same
/// directory — atomic, with no half-moved state. The prior conversation is
/// preserved (not deleted) as a one-slot recovery; a subsequent `--fresh`
/// replaces it. A crash after the rename but before respawn is fail-safe: the
/// next boot finds no JSONL and comes up fresh anyway, which is exactly the
/// requested intent. A non-`--fresh` boot never moves anything, so a session
/// the operator wanted to keep can never be lost by this path.
///
/// Returns the `.bak` path on a successful move, or `None` when there was no
/// session on disk (agent never ran, or already fresh).
pub fn freshen_session(
    claude_home: &Path,
    project: &str,
    agent: &str,
) -> io::Result<Option<PathBuf>> {
    let filename = format!("{}.jsonl", derive_session_id(project, agent));
    let projects_dir = claude_home.join("projects");
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            // Append `.bak` to the full filename — `with_extension` would
            // drop `.jsonl` and produce `<uuid>.bak`.
            let mut bak: OsString = candidate.clone().into_os_string();
            bak.push(".bak");
            let bak = PathBuf::from(bak);
            fs::rename(&candidate, &bak)?;
            return Ok(Some(bak));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_yield_same_uuid() {
        // Determinism is the core invariant — every claude-code spawn
        // for `(hello, mgr)` must hit the same on-disk session file.
        let a = derive_session_id("hello", "mgr");
        let b = derive_session_id("hello", "mgr");
        assert_eq!(a, b);
    }

    #[test]
    fn different_agents_yield_different_uuids() {
        // Agents within the same project must not collide on
        // session-id; if they did, two agents would share a single
        // claude conversation and stomp each other's context.
        let mgr = derive_session_id("hello", "mgr");
        let dev = derive_session_id("hello", "dev");
        assert_ne!(mgr, dev);
    }

    #[test]
    fn different_projects_yield_different_uuids() {
        // Cross-project isolation: same agent name in two different
        // projects must resolve to two different sessions.
        let a = derive_session_id("alpha", "mgr");
        let b = derive_session_id("beta", "mgr");
        assert_ne!(a, b);
    }

    #[test]
    fn namespace_constant_is_stable() {
        // Pin the namespace bytes so an accidental edit (anyone
        // tempted to "regenerate the constant for cleanliness")
        // surfaces here instead of silently invalidating every
        // existing session across every install.
        assert_eq!(
            TEAMCTL_SESSION_NAMESPACE.to_string(),
            "6dd6c8a3-44b6-4a18-9b05-91c1e257fb3d"
        );
    }

    #[test]
    fn derived_uuid_is_v5_and_uses_namespace() {
        // Cross-check against an independent re-computation so a
        // refactor of `derive_session_id` that accidentally changes
        // the namespace or the input shape can't pass.
        let derived = derive_session_id("hello", "mgr");
        let expected = Uuid::new_v5(&TEAMCTL_SESSION_NAMESPACE, b"teamctl:hello:mgr");
        assert_eq!(derived, expected);
        assert_eq!(derived.get_version_num(), 5);
    }

    #[test]
    fn session_name_format_is_canonical() {
        // The `-n <name>` value the operator sees in claude's session
        // picker should mirror the UUID input shape so a session-id
        // collision (would be astronomical, but) is debuggable from
        // the human-readable name alone.
        assert_eq!(session_name("hello", "mgr"), "teamctl:hello:mgr");
    }

    // ── T-352: freshen_session ───────────────────────────────────────

    /// Stage `<claude_home>/projects/<slug>/<uuid>.jsonl` for an agent and
    /// return its path. Mirrors Claude's on-disk layout the wrapper probes.
    fn stage_session(claude_home: &Path, slug: &str, project: &str, agent: &str) -> PathBuf {
        let dir = claude_home.join("projects").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl = dir.join(format!("{}.jsonl", derive_session_id(project, agent)));
        std::fs::write(&jsonl, "session-bytes").unwrap();
        jsonl
    }

    #[test]
    fn freshen_moves_existing_session_aside() {
        let home = tempfile::tempdir().unwrap();
        let jsonl = stage_session(home.path(), "-Users-x-proj", "hello", "mgr");

        let bak = freshen_session(home.path(), "hello", "mgr").unwrap();

        let bak = bak.expect("a staged session is reported moved");
        assert!(!jsonl.exists(), "original JSONL is gone after freshen");
        assert!(bak.exists(), "the .bak recovery copy exists");
        assert_eq!(bak.extension().unwrap(), "bak");
        // `.jsonl` is preserved before `.bak` (not clobbered by with_extension).
        assert!(bak.to_string_lossy().ends_with(".jsonl.bak"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "session-bytes");
    }

    #[test]
    fn freshen_is_noop_when_no_matching_session() {
        let home = tempfile::tempdir().unwrap();
        // A different agent's session is present; ours is not.
        stage_session(home.path(), "-Users-x-proj", "hello", "other");

        let bak = freshen_session(home.path(), "hello", "mgr").unwrap();
        assert!(bak.is_none(), "no move when our UUID has no JSONL on disk");
    }

    #[test]
    fn freshen_is_noop_when_projects_dir_absent() {
        let home = tempfile::tempdir().unwrap();
        // `~/.claude/projects` never created — agent never ran.
        let bak = freshen_session(home.path(), "hello", "mgr").unwrap();
        assert!(bak.is_none());
    }

    #[test]
    fn freshen_only_touches_the_session_jsonl() {
        // Durable on-disk state must survive --fresh. Stage a sibling file
        // next to the session and assert freshen leaves it alone.
        let home = tempfile::tempdir().unwrap();
        let jsonl = stage_session(home.path(), "-Users-x-proj", "hello", "mgr");
        let sibling = jsonl.with_file_name("task.md");
        std::fs::write(&sibling, "durable").unwrap();

        freshen_session(home.path(), "hello", "mgr")
            .unwrap()
            .unwrap();

        assert!(!jsonl.exists());
        assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "durable");
    }
}
