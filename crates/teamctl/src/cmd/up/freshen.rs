use super::*;

/// What a `--fresh` request resolves to for one agent, before any I/O.
/// Split out from [`freshen_for_spec`] so the runtime-gate decision —
/// the codex/gemini parity carve-out — is unit-testable without touching
/// the filesystem or `$HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshenAction {
    /// `--fresh` not set: do nothing.
    Skip,
    /// `--fresh` on a runtime with no mapped session model (gemini):
    /// warn and skip (parity gap).
    UnsupportedRuntime,
    /// `--fresh` on a Claude agent: move its session aside.
    Freshen,
    /// `--fresh` on a codex agent: move its per-agent CODEX_HOME
    /// sessions dir aside (`.bak` recovery copy) so the wrapper's
    /// resume probe falls through to a fresh spawn.
    WipeCodexSessions,
    /// `--fresh` on an opencode agent: move its per-agent session db
    /// (plus -shm/-wal sidecars) into a `db.bak/` recovery copy so the
    /// wrapper's resume probe falls through to a fresh spawn.
    WipeOpencodeSessions,
}

/// Resolve `(runtime, fresh)` to a [`FreshenAction`]. Pure — no I/O.
pub(crate) fn freshen_action(runtime: &str, fresh: bool) -> FreshenAction {
    if !fresh {
        FreshenAction::Skip
    } else if runtime == "claude-code" {
        FreshenAction::Freshen
    } else if runtime == "codex" {
        FreshenAction::WipeCodexSessions
    } else if runtime == "opencode" {
        FreshenAction::WipeOpencodeSessions
    } else {
        FreshenAction::UnsupportedRuntime
    }
}

/// T-352: when `--fresh` is set, drop the agent's session state just
/// before it (re)spawns so the wrapper opens a brand-new conversation
/// (re-running `BOOTSTRAP_PROMPT`). Claude: move the session JSONL
/// aside (same deterministic UUID, `.bak` recovery copy kept). Codex:
/// move the per-agent CODEX_HOME `sessions/` subdir to `sessions.bak`
/// (same recovery-copy parity) — the wrapper's resume probe then finds
/// no rollout and boots fresh; config.toml and the auth.json symlink
/// live at the codex-home root and survive. OpenCode: move the
/// per-agent session db (+ -shm/-wal sidecars) into `db.bak/` (same
/// parity) — the resume probe finds no db and boots fresh;
/// opencode.json at the home root survives.
/// Durable on-disk files are never touched.
///
/// Gemini has no session resume, so we warn-and-skip rather than abort
/// a mixed-runtime team (parity gap). Best-effort — a failure warns but
/// never blocks the respawn (coming up on the existing conversation is
/// strictly safer than refusing to start).
///
/// Call only for an agent that is actually being (re)spawned: freshening
/// an agent that won't respawn would move its live session aside with no
/// new conversation to replace it (a latent desync), so the callers gate
/// on "about to start this agent" before calling here.
pub(crate) fn freshen_for_spec(root: &Path, spec: &AgentSpec, runtime: &str, fresh: bool) {
    let id = format!("{}:{}", spec.project, spec.agent);
    match freshen_action(runtime, fresh) {
        FreshenAction::Skip => {}
        FreshenAction::UnsupportedRuntime => {
            eprintln!("warn · {id} (--fresh skipped: {runtime} runtime has no session resume yet)");
        }
        FreshenAction::Freshen => {
            let Some(home) = team_core::session::claude_home() else {
                eprintln!("warn · {id} (--fresh skipped: $HOME unset)");
                return;
            };
            if let Err(e) = team_core::session::freshen_session(&home, &spec.project, &spec.agent) {
                eprintln!("warn · {id} (--fresh: could not move session aside: {e})");
            }
        }
        FreshenAction::WipeCodexSessions => {
            // Move-aside, matching Claude's `.bak` recovery copy. An
            // absent dir just means "nothing to freshen" (first spawn,
            // or already fresh) — silently keep any earlier backup
            // instead of clobbering it with nothing.
            let home = codex_home_dir(root, &spec.project, &spec.agent);
            let sessions = home.join("sessions");
            if !sessions.exists() {
                return;
            }
            let bak = home.join("sessions.bak");
            if let Err(e) = fs::remove_dir_all(&bak) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("warn · {id} (--fresh: could not clear old sessions backup: {e})");
                }
            }
            if let Err(e) = fs::rename(&sessions, &bak) {
                eprintln!("warn · {id} (--fresh: could not move codex sessions aside: {e})");
            }
        }
        FreshenAction::WipeOpencodeSessions => {
            // Same move-aside parity as codex, adapted to opencode's
            // session store: one sqlite db (plus optional -shm/-wal
            // sidecars) instead of a directory tree. An absent db means
            // "nothing to freshen" — silently keep any earlier backup.
            // opencode.json at the home root is untouched.
            let home = opencode_home_dir(root, &spec.project, &spec.agent);
            // "Nothing to freshen" means db AND sidecars all absent —
            // gating on the db alone would strand an orphan -wal/-shm
            // left by a previous partial move.
            if ["agent.db", "agent.db-shm", "agent.db-wal"]
                .iter()
                .all(|n| !home.join(n).exists())
            {
                return;
            }
            let bak = home.join("db.bak");
            if let Err(e) = fs::remove_dir_all(&bak) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("warn · {id} (--fresh: could not clear old db backup: {e})");
                }
            }
            if let Err(e) = fs::create_dir_all(&bak) {
                eprintln!("warn · {id} (--fresh: could not create db backup dir: {e})");
                return;
            }
            for name in ["agent.db", "agent.db-shm", "agent.db-wal"] {
                let src = home.join(name);
                if !src.exists() {
                    continue; // sidecars only exist while WAL is active
                }
                if let Err(e) = fs::rename(&src, bak.join(name)) {
                    eprintln!("warn · {id} (--fresh: could not move {name} aside: {e})");
                }
            }
        }
    }
}

/// `" (fresh)"` when a `--fresh` restart is in effect, else empty. Kept a
/// free function so `up` and `reload` annotate their per-line logs and
/// dry-run output identically.
pub(crate) fn fresh_suffix(fresh: bool) -> &'static str {
    if fresh {
        " (fresh)"
    } else {
        ""
    }
}
