use super::*;

/// Executable descriptor adapter, shared by built-in and custom runtimes.
pub fn runtime_launcher_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/runtimes")
        .join(format!("{project}-{agent}.sh"))
}

/// Absolute path to the rendered env file for a given agent.
pub fn env_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/envs")
        .join(format!("{project}-{agent}.env"))
}

/// Absolute path to the rendered MCP config for a given agent.
pub fn mcp_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/mcp")
        .join(format!("{project}-{agent}.json"))
}

/// Absolute path to the wrapper-managed Claude Code settings file. The
/// file carries the default `PreToolUse` deny hook for synchronous-prompt
/// tools (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`) so a
/// headless agent doesn't strand on a picker no one will answer. The
/// wrapper applies it via `claude --settings <path>` for every
/// claude-code agent except those in `permission_mode: attended`.
pub fn claude_settings_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/claude")
        .join(format!("{project}-{agent}.json"))
}

/// Absolute path to the rendered Claude Code `--agents` JSON for one agent
/// (#383 Phase 3a). Lives beside the settings file under `state/claude/`
/// and is written only when the agent declares `subagents:`; the wrapper
/// passes it via `--agents "$(cat <path>)"` when the file exists.
pub fn subagents_json_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/claude")
        .join(format!("{project}-{agent}.agents.json"))
}

/// Absolute path to the per-agent scope directory passed to Claude Code
/// via `--add-dir` (#383 Phase 3b). render materializes
/// `<this>/.claude/skills/<name>` symlinks to each declared skill; the
/// wrapper adds `--add-dir <this>` so the agent discovers them on top of
/// the project `.claude/skills/`. The directory is materialized only when
/// the agent declares `skills:`; the wrapper's `[ -d ]` guard decides
/// whether the flag is passed.
pub fn agent_scope_dir(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/agent-scope")
        .join(format!("{project}-{agent}"))
}

/// Absolute path to the materialized concatenation of a multi-file
/// `role_prompt` list. Only ever written for the list form — single-file
/// `role_prompt` keeps pointing at its source path directly.
pub fn role_prompt_concat_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/role_prompts")
        .join(format!("{project}-{agent}.md"))
}

/// Absolute path to the per-agent activity heartbeat marker (#428). The
/// `PreToolUse`/`UserPromptSubmit` hooks `touch` it on activity and the
/// `Stop`/`StopFailure` hooks `rm` it at turn-end; the TUI `stat`s its
/// mtime at the 1s refresh and classifies the agent Working (touched
/// within 15s) or Idle. NOT JSON — a bare marker whose mtime is the whole
/// signal. Compound `<project>-<agent>` like every sibling helper, so
/// agents that share a name across projects never collide on one marker.
pub fn heartbeat_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/heartbeats")
        .join(format!("{project}-{agent}"))
}

/// Per-agent "last seen" marker, a sibling of [`heartbeat_path`] (#439). The
/// boot-context hook `touch`es it at clean turn-end — alongside `rm`-ing the
/// heartbeat marker — so a freshly woken session can compute how long the
/// agent was down. Unlike the heartbeat marker (removed at every turn-end,
/// so present at boot only after an *unclean* shutdown), this one persists
/// across the gap, making its mtime the agent's last activity. Same compound
/// `<project>-<agent>` stem as every sibling so cross-project name clashes
/// can't collide, with a `.lastseen` suffix so it never shadows the marker
/// the TUI stats for Working/Idle.
pub fn lastseen_path(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/heartbeats")
        .join(format!("{project}-{agent}.lastseen"))
}

/// Absolute path to the shared boot-context hook script (#430). Wired into
/// every claude-code agent's `SessionStart` hook and rewritten by `teamctl
/// up` (see `ensure_wrapper_and_dirs`), so it sits beside the agent wrapper
/// in `bin/` rather than under per-agent `state/`. One script serves all
/// agents — it reads the wake `source` from stdin, so it needs no per-agent
/// identity baked into the path.
pub fn boot_script_path(root: &Path) -> PathBuf {
    root.join("bin/boot.sh")
}

/// Absolute path to the per-agent Codex home directory. `CODEX_HOME`
/// relocates codex's entire state root — config, sessions, history — so
/// pointing each codex agent at its own rendered home is the clean
/// per-process isolation mechanism: MCP tables and instructions can't
/// collide across agents. render writes `<this>/config.toml`; the wrapper
/// exports `CODEX_HOME=<this>` and symlinks the operator's `auth.json` in
/// so agents share the existing login.
pub fn codex_home_dir(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/codex-home")
        .join(format!("{project}-{agent}"))
}

/// Absolute path to the per-agent OpenCode home directory. OpenCode has
/// no single state-root env var like `CODEX_HOME`; per-agent isolation
/// is two env vars pointing into this rendered dir instead:
/// `OPENCODE_DB` relocates the session sqlite db (created beside its
/// -shm/-wal sidecars) and `OPENCODE_CONFIG` loads the per-process
/// config json carrying the MCP servers + instructions. render writes
/// `<this>/opencode.json`; the wrapper's env file points both vars here.
/// Credentials are NOT relocated — auth stays at the operator's real
/// `~/.local/share/opencode/auth.json`, shared by every agent.
pub fn opencode_home_dir(root: &Path, project: &str, agent: &str) -> PathBuf {
    root.join("state/opencode-home")
        .join(format!("{project}-{agent}"))
}
