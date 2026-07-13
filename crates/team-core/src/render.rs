//! Render a loaded compose into on-disk artifacts.
//!
//! Outputs under `<root>/state/`:
//! - `envs/<project>-<agent>.env`      — env vars for the agent wrapper.
//! - `mcp/<project>-<agent>.json`      — MCP stdio config for the runtime.
//! - `claude/<project>-<agent>.json`   — wrapper-managed Claude Code
//!   settings (currently a `PreToolUse` deny hook for synchronous-prompt
//!   tools that strand a headless pane). Claude-code agents only.
//! - `codex-home/<project>-<agent>/config.toml` — per-agent Codex home
//!   carrying the same MCP servers as the JSON above in the
//!   `[mcp_servers.<name>]` table form codex reads (codex has no
//!   `--mcp-config` flag). Codex agents only.
//! - `role_prompts/<project>-<agent>.md` (multi-file role_prompt only) —
//!   the ordered concatenation of every source file declared in the
//!   role's `role_prompt: [...]` list. Re-materialized on every render
//!   so any source-file edit lands in the agent's prompt at next boot.
//!
//! `systemd` / `launchd` unit rendering lives behind a feature flag when
//! those back-ends are enabled via `supervisor.type`.

use std::io;
use std::path::{Path, PathBuf};

use crate::compose::{AgentHandle, Compose, RolePrompt};

/// Separator written between concatenated role-prompt files. Em-dash
/// framed by blank lines reads cleanly when an operator inspects the
/// materialized file under `state/role_prompts/`.
const ROLE_PROMPT_SEPARATOR: &str = "\n\n—\n\n";

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

/// Rendered env + MCP content for a single agent.
pub fn render_agent(
    compose: &Compose,
    handle: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> (String, String) {
    let env = render_env(compose, handle);
    let mcp = render_mcp(compose, handle, team_mcp_bin);
    (env, mcp)
}

/// Wrapper-managed Claude Code settings JSON for a single agent. Returns
/// `Some(json)` for `claude-code` runtime regardless of `permission_mode`
/// — the wrapper decides whether to apply it. Returns `None` for runtimes
/// that don't read Claude settings (codex, gemini, …).
///
/// The base payload is a single `PreToolUse` deny hook covering the
/// synchronous-prompt tools that today strand a headless pane:
/// `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`. The `systemMessage`
/// tells the model *why* the deny fired and points it at the `team` MCP
/// tools as the headless-safe alternative — without that, the model just
/// sees the call vanish and may retry. Matcher is a regex; extend it
/// (rather than the hook count) when claude-code gains new synchronous-
/// prompt tools.
///
/// #383 Phase 2: per-agent hooks declared in compose (`Agent.hooks`) are
/// merged on top of that base. Each declaration is appended as its own
/// entry under its event, so the built-in deny hook keeps its slot and a
/// user hook can extend behavior but not clobber the interactive-prompt
/// deny. Hook commands are compose-root-relative and rendered absolute.
pub fn render_claude_settings(compose: &Compose, h: AgentHandle<'_>) -> Option<String> {
    if h.spec.runtime != "claude-code" {
        // Hooks are a Claude-Code concept. On other runtimes the whole
        // settings file is skipped; surface a warning so a declared-but-
        // ignored hook isn't silently dropped (claude-only v1).
        if !h.spec.hooks.is_empty() {
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} hook(s) but runtime `{}` does not support hooks (claude-code only); ignoring",
                h.project,
                h.agent,
                h.spec.hooks.len(),
                h.spec.runtime
            );
        }
        // #461: same degrade for ultracode — a declared opt-in on a runtime
        // that doesn't read claude settings is a no-op, so warn rather than
        // silently drop it (matches the hooks/skills/subagents pattern).
        if h.spec.ultracode {
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` sets ultracode but runtime `{}` does not support it (claude-code only); ignoring",
                h.project,
                h.agent,
                h.spec.runtime
            );
        }
        return None;
    }
    // PreToolUse deny hook. Picked over `--disallowed-tools` so the
    // model sees the deny + systemMessage (tighter learning loop) rather
    // than the tool silently vanishing from its catalog. Emitted first
    // and never removed; declared hooks (below) are appended after it.
    let mut v = serde_json::json!({
        // #421: pre-trust every project-scoped MCP server for headless
        // agents. When Claude Code discovers a `.mcp.json` server it hasn't
        // seen — on a fresh session or after `update` introduces a new one —
        // it otherwise blocks on a "New MCP server found in this project:
        // <name>" prompt. An unattended agent has no human to press Enter, so
        // it freezes indefinitely (live owner repro). This top-level key
        // pre-approves all *project* MCP servers (not user/global), so the
        // prompt never fires. It only reaches headless agents: attended
        // sessions skip `--settings` entirely, so a human at the terminal
        // still sees and answers the prompt — a built-in opt-out. The key is
        // Claude-owned: verified working against Claude Code 2.1.165, but a
        // future rename would silently no-op and re-freeze headless panes, so
        // the startup-dialog watcher stays as a version-independent backstop.
        // Trade-off the owner OK'd: unattended convenience over per-server
        // confirmation, scoped to this project's declared servers.
        "enableAllProjectMcpServers": true,
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "AskUserQuestion|EnterPlanMode|ExitPlanMode",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\"},\"systemMessage\":\"Interactive prompts are disabled for teamctl agents. Use the `team` MCP tools to ask people or check in.\"}'"
                        }
                    ]
                }
            ]
        }
    });

    // #383 Phase 2: merge per-agent declared hooks on top. Each
    // declaration becomes its own entry appended to its event's array, so
    // the built-in deny hook above always keeps its slot. Commands are
    // compose-root-relative (like `role_prompt`), rendered as absolute
    // paths.
    let hooks_obj = v["hooks"].as_object_mut().expect("hooks is a json object");

    // #428: per-agent activity heartbeat. The TUI derives a Working/Idle
    // sub-state of `Running` from the mtime of a per-agent marker file
    // (touched within 15s => Working) — see `heartbeat_path` and
    // `teamctl-ui`'s `data::is_working`. `PreToolUse` + `UserPromptSubmit`
    // `touch` the marker on every tool call / prompt; `Stop` + `StopFailure`
    // `rm` it at turn-end. No `matcher` => match all tools (do NOT borrow
    // the deny hook's narrow matcher). The marker path is shell-quoted via
    // `shlex` (not hand-rolled) so a compose root with spaces OR an embedded
    // quote can't word-split and silently touch/rm the wrong path. The
    // commands emit no stdout — that matters for `UserPromptSubmit`, whose
    // exit-0 stdout is injected into the model's context. Zero DB writes:
    // the hook only touches a file the TUI stat()s. The `state/heartbeats/`
    // dir is created by `teamctl up`/`reload` alongside the other state
    // subdirs, so the command is a bare `touch`. (A marker left fresh by an
    // unclean shutdown is bounded to one 15s window and masked by the
    // Stopped/Unknown state gate in the roster — see #428 / the PR note.)
    {
        let path = heartbeat_path(&compose.root, h.project, h.agent)
            .display()
            .to_string();
        // Reuse the crate's POSIX single-quote escaper (errors only on a NUL
        // byte, impossible in a filesystem path) rather than hand-rolling
        // quoting that breaks on an embedded apostrophe.
        let marker =
            crate::supervisor::shlex::try_quote(&path).expect("heartbeat marker path is NUL-free");
        // #439: the turn-end clear first `touch`es the per-agent LASTSEEN
        // sibling, recording the moment of last activity before removing the
        // marker. LASTSEEN survives the gap (the marker does not), so the
        // boot-context hook can read its mtime to report downtime on the next
        // startup. `touch && rm` keeps it one command CC runs in one /bin/sh;
        // the touch is on a dir teamctl guarantees exists, so it effectively
        // never fails — and if it ever did, the marker simply lingers one 15s
        // window (the same bound an unclean shutdown already carries).
        let lastseen_p = lastseen_path(&compose.root, h.project, h.agent)
            .display()
            .to_string();
        let lastseen = crate::supervisor::shlex::try_quote(&lastseen_p)
            .expect("lastseen marker path is NUL-free");
        let touch = format!("touch {marker}");
        let clear = format!("touch {lastseen} && rm -f {marker}");
        for (event, command) in [
            ("PreToolUse", &touch),
            ("UserPromptSubmit", &touch),
            ("Stop", &clear),
            ("StopFailure", &clear),
        ] {
            hooks_obj
                .entry(event.to_string())
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .expect("hook event maps to a json array")
                .push(serde_json::json!({
                    "hooks": [ { "type": "command", "command": command } ]
                }));
        }
    }

    // #430: boot-context SessionStart hook. On every session (re)start Claude
    // Code runs this and injects the script's stdout (`additionalContext`)
    // into the agent's context, so a freshly woken pane knows it just
    // (re)started and from which transition. The command is the shared
    // `bin/boot.sh` asset `teamctl up` emits: inlining it would mean
    // triple-escaping a sed + case + JSON-emit pipeline through shell ×
    // settings-JSON × Rust, and it runs in the agent's `/bin/sh` (macOS bash
    // 3.2), so a real file stays readable and `sh -n`-checkable. No `matcher`
    // => fire on every source (startup|resume|clear|compact); `timeout: 5`
    // bounds a wedged hook. The script emits the REQUIRED `hookEventName`
    // itself — without it Claude Code silently drops `additionalContext`, the
    // exact trap this hook exists to avoid. The path is shlex-quoted (like the
    // #428 marker) so a compose root with spaces or a quote can't word-split.
    // Supersedes the bootstrap-prompt mechanism #258 sketched (do not close
    // #258 — its downtime-context idea lives on here).
    {
        let path = boot_script_path(&compose.root).display().to_string();
        let boot =
            crate::supervisor::shlex::try_quote(&path).expect("boot script path is NUL-free");
        // #439: pass the per-agent LASTSEEN + MARKER paths as positional argv
        // so boot.sh can report downtime on `startup`. The script stays shared
        // and agent-agnostic — identity arrives via argv, not baked into the
        // path (the #428 per-agent precedent). Each path is shlex-quoted like
        // the script path, so a compose root with spaces or a quote can't
        // word-split into the wrong argument. Order is (lastseen, marker);
        // boot.sh prefers the marker's mtime when it survives an unclean stop.
        let lastseen_p = lastseen_path(&compose.root, h.project, h.agent)
            .display()
            .to_string();
        let lastseen = crate::supervisor::shlex::try_quote(&lastseen_p)
            .expect("lastseen marker path is NUL-free");
        let marker_p = heartbeat_path(&compose.root, h.project, h.agent)
            .display()
            .to_string();
        let marker = crate::supervisor::shlex::try_quote(&marker_p)
            .expect("heartbeat marker path is NUL-free");
        let command = format!("{boot} {lastseen} {marker}");
        hooks_obj
            .entry("SessionStart".to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("hook event maps to a json array")
            .push(serde_json::json!({
                "hooks": [ { "type": "command", "command": command, "timeout": 5 } ]
            }));
    }

    // #431: rate-limit hit marker. Appended as a SECOND entry to the same
    // `StopFailure` bucket the #428 heartbeat clear already lives in (slot 0 =
    // match-all `rm -f <marker>`; this is slot 1, scoped by `matcher`). On a
    // turn that ends because the runtime hit its rate limit, Claude Code runs
    // this and `teamctl rl-hit` records a forensic hit row. The `rate_limit`
    // matcher is a real `StopFailure` reason value, so the entry only fires on
    // rate-limit stops, not every failure. The command mirrors the wrapper's
    // own convention (agent-wrapper.sh): a PATH `teamctl` guarded by
    // `command -v`, with a trailing `|| true` so this is pure fire-and-forget:
    // a host without teamctl on PATH (or any rl-hit error) degrades to a silent
    // exit-0 no-op instead of erroring the stop, matching the heartbeat clear's
    // always-exit-0 `rm -f`. The compose root and the
    // `<project>:<agent>` id are baked in (render has both in scope, no env
    // dependency) and shlex-quoted like the #428 marker / #430 boot path; the
    // guard and the `--root`/`rl-hit` literals are not quoted. The hook has no
    // PTY output to read a reset time from, so `rl-hit` stores `resets_at` NULL,
    // invisible to the TUI countdown (which filters `resets_at IS NOT NULL`),
    // leaving `rl-watch` the sole countdown source.
    {
        let root = crate::supervisor::shlex::try_quote(&compose.root.display().to_string())
            .expect("compose root is NUL-free");
        let agent_id = format!("{}:{}", h.project, h.agent);
        let agent_id =
            crate::supervisor::shlex::try_quote(&agent_id).expect("agent id is NUL-free");
        let command = format!(
            "command -v teamctl >/dev/null 2>&1 && teamctl --root {root} rl-hit {agent_id} || true"
        );
        hooks_obj
            .entry("StopFailure".to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("hook event maps to a json array")
            .push(serde_json::json!({
                "matcher": "rate_limit",
                "hooks": [ { "type": "command", "command": command } ]
            }));
    }

    // #333: budget cost writer. Appended as a SECOND entry to the same `Stop`
    // bucket the #428 heartbeat clear already lives in (slot 0 = match-all
    // touch-lastseen + `rm -f <marker>`; this is slot 1, also match-all so it
    // fires on every clean turn-end). Claude Code pipes the Stop payload to
    // this command on stdin; `teamctl budget-record` reads the transcript named
    // in that payload, sums the just-finished turn's token usage, prices it, and
    // INSERTs one `budget` row — the missing writer behind a permanently-$0.00
    // `USD-24H`. The command mirrors the #431 rl-hit shape exactly: a PATH
    // `teamctl` guarded by `command -v`, with a trailing `|| true` so it's pure
    // fire-and-forget — a host without teamctl on PATH (or any record error)
    // degrades to a silent exit-0 no-op instead of erroring the stop, matching
    // the heartbeat clear's always-exit-0 `rm -f`. The compose root and the
    // `<project>:<agent>` id are baked in (render has both in scope, no env
    // dependency) and shlex-quoted like the #428 marker / #431 rl-hit id; the
    // guard and the `--root`/`budget-record` literals are not quoted. On `Stop`
    // (clean turn-end) only: a rate-limited turn ends on `StopFailure`, so its
    // partial spend is intentionally not recorded — an accepted v1 gap.
    {
        let root = crate::supervisor::shlex::try_quote(&compose.root.display().to_string())
            .expect("compose root is NUL-free");
        let agent_id = format!("{}:{}", h.project, h.agent);
        let agent_id =
            crate::supervisor::shlex::try_quote(&agent_id).expect("agent id is NUL-free");
        let command = format!(
            "command -v teamctl >/dev/null 2>&1 && teamctl --root {root} budget-record {agent_id} || true"
        );
        hooks_obj
            .entry("Stop".to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("hook event maps to a json array")
            .push(serde_json::json!({
                "hooks": [ { "type": "command", "command": command } ]
            }));
    }

    for hook in &h.spec.hooks {
        let command = compose.root.join(&hook.command);
        let mut entry = serde_json::json!({
            "hooks": [
                {
                    "type": "command",
                    "command": command.display().to_string()
                }
            ]
        });
        if let Some(matcher) = &hook.matcher {
            entry["matcher"] = serde_json::Value::String(matcher.clone());
        }
        hooks_obj
            .entry(hook.event.clone())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("hook event maps to a json array")
            .push(entry);
    }

    // #461: per-agent ultracode opt-in. ultracode is a Claude Code settings
    // key (verified against 2.1.175: settable via `--settings '{"ultracode":
    // true}'`; NOT a CLI flag and NOT an effort value), so it rides this same
    // settings file the wrapper passes via `--settings`. Inserted only when
    // opted in, so the default settings shape is byte-identical for everyone
    // else. claude-only falls out for free: this fn already returned `None`
    // above for non-claude runtimes.
    if h.spec.ultracode {
        v["ultracode"] = serde_json::Value::Bool(true);
    }

    Some(serde_json::to_string_pretty(&v).expect("json"))
}

/// #383 Phase 3a: build Claude Code's `--agents` inline JSON for one agent
/// from its declared `subagents:` list. Each list entry is a
/// compose-root-relative markdown file with standard sub-agent frontmatter
/// (`name`, `description`, optional `tools`, `model`) and a body that
/// becomes the sub-agent's system `prompt`. The result is the
/// `{ "<name>": { description, prompt, [tools], [model] } }` object the
/// `--agents` flag consumes — the only cwd-stationary way to scope
/// sub-agents per agent (no arbitrary-path flag exists; see the Phase-1
/// spike). Returns `Ok(None)` when none are declared (→ no `--agents`
/// flag) or the runtime isn't claude-code (logs an "unsupported" warning,
/// claude-only v1); `Err` if a source is unreadable or its frontmatter is
/// invalid, so a typo fails the apply loudly rather than dropping a
/// sub-agent silently.
pub fn render_subagents(compose: &Compose, h: AgentHandle<'_>) -> io::Result<Option<String>> {
    if h.spec.subagents.is_empty() {
        return Ok(None);
    }
    if h.spec.runtime != "claude-code" {
        tracing::warn!(
            target: "team-core::render",
            "agent `{}:{}` declares {} sub-agent(s) but runtime `{}` does not support sub-agents (claude-code only); ignoring",
            h.project,
            h.agent,
            h.spec.subagents.len(),
            h.spec.runtime
        );
        return Ok(None);
    }

    let mut map = serde_json::Map::new();
    for rel in &h.spec.subagents {
        let abs = compose.root.join(rel);
        let raw = std::fs::read_to_string(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read sub-agent source {}: {e}", abs.display()),
            )
        })?;
        let (fm, body) = parse_subagent(&raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse sub-agent {}: {e}", abs.display()),
            )
        })?;
        // Name from frontmatter, else the file stem (so `agents/foo.md`
        // without an explicit `name:` registers as sub-agent `foo`).
        let name = fm.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            rel.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let mut entry = serde_json::json!({
            "description": fm.description,
            "prompt": body,
        });
        if let Some(tools) = fm.tools {
            let list = tools.into_list();
            if !list.is_empty() {
                entry["tools"] = serde_json::json!(list);
            }
        }
        if let Some(model) = fm.model.filter(|m| !m.trim().is_empty()) {
            entry["model"] = serde_json::Value::String(model);
        }
        map.insert(name, entry);
    }
    Ok(Some(
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).expect("json"),
    ))
}

/// Write (or clear) the per-agent `--agents` JSON file. Mirrors
/// [`write_role_prompt_concat`]: the scoped + full render paths both call
/// it so a `subagents:` edit flows into the agent at the next render. When
/// the agent declares no sub-agents (or isn't claude-code) the file is
/// removed if present, so a stale `--agents` set never lingers across a
/// reload that dropped them.
pub fn write_subagents_json(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let dest = subagents_json_path(&compose.root, h.project, h.agent);
    match render_subagents(compose, h)? {
        Some(json) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, json)
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

/// Materialize (or clear) the per-agent skills scope for one agent (#383
/// Phase 3b). For a claude-code agent declaring `skills:`, this creates
/// `state/agent-scope/<project>-<agent>/.claude/skills/` and symlinks each
/// declared skill directory into it (link name = the skill dir's basename),
/// so `claude --add-dir <scope>` surfaces them additively atop the project
/// `.claude/skills/`. Mirrors [`write_subagents_json`]: the scoped + full
/// render paths both call it, and the skills dir is rebuilt from scratch
/// every render so a renamed or dropped skill never lingers. When the agent
/// declares no skills (or isn't claude-code) the scope dir is removed if
/// present.
///
/// Symlink targets are absolute (compose-root-relative input resolved
/// against `compose.root`); a missing source becomes a dangling link rather
/// than an error, matching how `role_prompt`/`hooks` treat not-yet-created
/// paths (existence checks across all path-typed fields are a tracked
/// follow-up). Clearing always unlinks entries individually — render never
/// hands a symlink to `remove_dir_all`, so a skill's real files are never
/// followed or deleted.
pub fn write_agent_skills(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let scope = agent_scope_dir(&compose.root, h.project, h.agent);
    let skills_dir = scope.join(".claude/skills");

    if h.spec.runtime != "claude-code" || h.spec.skills.is_empty() {
        if h.spec.runtime != "claude-code" && !h.spec.skills.is_empty() {
            // Skills are a Claude-Code concept; surface a warning so a
            // declared-but-ignored skill isn't silently dropped (claude-
            // only v1, same shape as hooks/sub-agents).
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} skill(s) but runtime `{}` does not support skills (claude-code only); ignoring",
                h.project,
                h.agent,
                h.spec.skills.len(),
                h.spec.runtime
            );
        }
        // Clear a stale scope dir so dropped skills don't linger across a
        // reload that removed them.
        return remove_scope_dir(&scope);
    }

    // Rebuild from scratch each render: clear the existing links (each is a
    // symlink we created — unlink it, never recurse into its target) then
    // re-create the current set.
    clear_skills_dir(&skills_dir)?;
    std::fs::create_dir_all(&skills_dir)?;
    for rel in &h.spec.skills {
        // Link name is the skill directory's basename — Claude Code
        // discovers `.claude/skills/<name>/SKILL.md`.
        let Some(name) = rel.file_name() else {
            continue; // path ending in `..` / root has no skill name
        };
        let link = skills_dir.join(name);
        // Last-wins on a duplicate basename (consistent with sub-agents'
        // name-keyed map): drop any link already placed for this name.
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(compose.root.join(rel), &link)?;
    }
    Ok(())
}

/// Remove the per-agent scope dir if present. Clears the managed symlinks
/// individually first, so `remove_dir_all` only ever sees plain
/// directories — it never gets a symlink entry that could be followed into
/// a skill's real files. No-op when the dir doesn't exist.
fn remove_scope_dir(scope: &Path) -> io::Result<()> {
    clear_skills_dir(&scope.join(".claude/skills"))?;
    match std::fs::remove_dir_all(scope) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove every entry in the per-agent skills dir. Each entry is a symlink
/// render created, so we `remove_file` (unlink) it — never recursing into
/// the skill's real contents. No-op when the dir doesn't exist yet.
fn clear_skills_dir(skills_dir: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() || meta.is_file() {
            std::fs::remove_file(&path)?;
        } else {
            // Defensive: we only create symlinks here, but if a real
            // subdir somehow appears, clear it without following links.
            std::fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// Parsed frontmatter of a sub-agent markdown file. Mirrors the fields
/// Claude Code's own `.claude/agents/*.md` use; unknown keys are ignored.
#[derive(serde::Deserialize)]
struct SubagentFrontmatter {
    #[serde(default)]
    name: Option<String>,
    description: String,
    #[serde(default)]
    tools: Option<Tools>,
    #[serde(default)]
    model: Option<String>,
}

/// `tools:` accepts either Claude Code's comma-separated string form
/// (`Read, Grep`) or a YAML list (`[Read, Grep]`); both normalize to the
/// JSON array `--agents` expects.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Tools {
    List(Vec<String>),
    Csv(String),
}

impl Tools {
    fn into_list(self) -> Vec<String> {
        let raw = match self {
            Tools::List(v) => v,
            Tools::Csv(s) => s.split(',').map(str::to_string).collect(),
        };
        raw.into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Split a sub-agent markdown file into (frontmatter, body). Expects the
/// standard `---\n<yaml>\n---\n<body>` layout; the body is everything after
/// the closing delimiter, trimmed of surrounding blank lines.
fn parse_subagent(raw: &str) -> Result<(SubagentFrontmatter, String), String> {
    let after_open = raw
        .strip_prefix("---")
        .ok_or("missing opening `---` frontmatter delimiter")?;
    let (yaml, body) = after_open
        .split_once("\n---")
        .ok_or("missing closing `---` frontmatter delimiter")?;
    let fm: SubagentFrontmatter =
        serde_yaml::from_str(yaml.trim()).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;
    let body = body.trim_start_matches(['\r', '\n']).trim_end().to_string();
    Ok((fm, body))
}

fn render_env(compose: &Compose, h: AgentHandle<'_>) -> String {
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .expect("agent belongs to a loaded project");
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mcp = mcp_path(&compose.root, h.project, h.agent);
    let prompt = system_prompt_path(compose, h)
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut s = String::new();
    s.push_str(&format!("AGENT_ID={}:{}\n", h.project, h.agent));
    s.push_str(&format!("PROJECT_ID={}\n", h.project));
    s.push_str(&format!("RUNTIME={}\n", h.spec.runtime));
    if let Some(m) = &h.spec.model {
        s.push_str(&format!("MODEL={m}\n"));
    }
    if let Some(pm) = &h.spec.permission_mode {
        s.push_str(&format!("PERMISSION_MODE={pm}\n"));
    }
    // T-048: per-agent reasoning effort flows through to the runtime
    // via the wrapper. Workspace-level `.env` `EFFORT=` still wins for
    // operators not yet on the YAML form (back-compat).
    if let Some(effort) = h.spec.effort {
        s.push_str(&format!("EFFORT={}\n", effort.as_str()));
    }
    s.push_str(&format!("TEAMCTL_MAILBOX={}\n", mailbox.display()));
    s.push_str(&format!("MCP_CONFIG={}\n", mcp.display()));
    s.push_str(&format!("SYSTEM_PROMPT_PATH={prompt}\n"));
    s.push_str(&format!(
        "CLAUDE_PROJECT_DIR={}\n",
        project.project.cwd.display()
    ));
    // Absolute path to the compose root (the directory holding
    // `team-compose.yaml`). The wrapper passes this to `teamctl --root`
    // so rl-watch resolves the right tree regardless of where
    // `cd "$CLAUDE_PROJECT_DIR"` lands the shell. Without this,
    // wrapper falls back to CLAUDE_PROJECT_DIR (often a relative `..`)
    // which compounds with the post-cd cwd and points at the wrong
    // directory.
    s.push_str(&format!("TEAMCTL_ROOT={}\n", compose.root.display()));
    s.push_str(&format!(
        "TMUX_SESSION={}{}-{}\n",
        compose.global.supervisor.tmux_prefix, h.project, h.agent
    ));
    // T-118: claude-code agents resume their conversation across
    // teamctl down/up + crash recovery via a deterministic UUIDv5
    // session id. Other runtimes don't recognize `--session-id`, so
    // emit these env vars only for `claude-code` — the wrapper's
    // claude-code arm picks them up; other arms ignore them.
    if h.spec.runtime == "claude-code" {
        let session_id = crate::session::derive_session_id(h.project, h.agent);
        let session_name = crate::session::session_name(h.project, h.agent);
        s.push_str(&format!("CLAUDE_SESSION_ID={session_id}\n"));
        s.push_str(&format!("CLAUDE_SESSION_NAME={session_name}\n"));
        // T-189: path to the wrapper-managed Claude settings file
        // carrying the synchronous-prompt deny hook. Wrapper applies
        // it via `--settings` except when `permission_mode: attended`
        // (human at the keyboard wants the interactive tools back).
        let settings = claude_settings_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_SETTINGS={}\n", settings.display()));
        // #383 Phase 3a: path to the rendered `--agents` JSON carrying this
        // agent's declared sub-agents. Always emitted for claude-code; the
        // file itself is written only when `subagents:` is non-empty, so
        // the wrapper's `[ -f ]` guard decides whether `--agents` is passed.
        let subagents = subagents_json_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_AGENTS_JSON={}\n", subagents.display()));
        // #383 Phase 3b: path to the per-agent skills scope dir passed to
        // `claude --add-dir`. Always emitted for claude-code; the dir is
        // materialized only when `skills:` is non-empty, so the wrapper's
        // `[ -d ]` guard decides whether `--add-dir` is passed.
        let scope = agent_scope_dir(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_AGENT_SCOPE={}\n", scope.display()));
    }
    // Codex has no `--mcp-config` flag — its MCP servers live in
    // `[mcp_servers.*]` tables inside `$CODEX_HOME/config.toml`, and
    // `CODEX_HOME` relocates codex's whole state root. Point each codex
    // agent at its own rendered home (written by [`write_codex_config`])
    // so the wrapper can export it; other runtimes must not see the var.
    if h.spec.runtime == "codex" {
        let home = codex_home_dir(&compose.root, h.project, h.agent);
        s.push_str(&format!("CODEX_HOME={}\n", home.display()));
    }
    s
}

/// Resolve the absolute path that `SYSTEM_PROMPT_PATH` will point at.
///
/// - `None` role_prompt → `None` (env line renders as blank).
/// - Single source file → `<root>/<source>` (back-compat, no concat
///   file is written — the operator's source is the prompt).
/// - List form → the materialized concat path under
///   `<root>/state/role_prompts/<project>-<agent>.md`. The file at that
///   path is produced by [`write_role_prompt_concat`]; this helper is
///   pure and only computes the destination.
pub fn system_prompt_path(compose: &Compose, h: AgentHandle<'_>) -> Option<PathBuf> {
    match h.spec.role_prompt.as_ref()? {
        RolePrompt::Single(p) => Some(compose.root.join(p)),
        RolePrompt::Multiple(_) => Some(role_prompt_concat_path(&compose.root, h.project, h.agent)),
    }
}

/// Materialize the multi-file `role_prompt` concatenation for one agent.
///
/// No-op when `role_prompt` is `None` or `Single` — there is nothing to
/// concatenate. For the list form, every source file is read in declared
/// order and joined with [`ROLE_PROMPT_SEPARATOR`]; the result overwrites
/// `<root>/state/role_prompts/<project>-<agent>.md` so subsequent edits
/// to any source file flow into the agent's prompt at the next render.
///
/// Missing source files surface as the underlying `io::Error` so the
/// caller can fail the apply rather than silently emit a partial concat.
pub fn write_role_prompt_concat(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let Some(RolePrompt::Multiple(paths)) = h.spec.role_prompt.as_ref() else {
        return Ok(());
    };

    let mut buf = String::new();
    for (idx, rel) in paths.iter().enumerate() {
        if idx > 0 {
            buf.push_str(ROLE_PROMPT_SEPARATOR);
        }
        let abs = compose.root.join(rel);
        let bytes = std::fs::read(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read role_prompt source {}: {e}", abs.display()),
            )
        })?;
        // Source files are expected to be UTF-8 markdown; lossy decode
        // keeps render diagnostics readable if a stray byte sneaks in.
        buf.push_str(&String::from_utf8_lossy(&bytes));
    }

    let dest = role_prompt_concat_path(&compose.root, h.project, h.agent);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, buf)
}

/// Args for the built-in `team` MCP stdio server. Single source of truth
/// shared by [`render_mcp`] (JSON) and [`render_codex_config`] (TOML in
/// the per-agent `CODEX_HOME`) so the two transports can never drift.
fn team_server_args(compose: &Compose, h: AgentHandle<'_>) -> Vec<String> {
    let mailbox = compose.root.join(&compose.global.broker.path);
    vec![
        "--agent-id".into(),
        format!("{}:{}", h.project, h.agent),
        "--mailbox".into(),
        mailbox.display().to_string(),
        // T-109: compact_self resolves the caller's tmux pane
        // as `<prefix><project>-<agent>`. Pass the configured
        // prefix explicitly so teams overriding the default
        // (`a-`, `oss-`, …) route the slash command to the
        // right session. team-bot gets the same arg threaded
        // from `teamctl bot up`; this keeps the two MCP-side
        // and bot-side resolvers in sync.
        "--tmux-prefix".into(),
        compose.global.supervisor.tmux_prefix.clone(),
        // T-32b: compose root used by `read_attachment`
        // for `attachments:` policy + tempfile staging.
        // Always passed so the per-agent team-mcp can
        // serve attachment reads; the staging dir is
        // computed under this root.
        "--compose-root".into(),
        compose.root.display().to_string(),
    ]
}

/// Whether declared `mcps:` render for this agent's runtime. Fail open
/// when the descriptor is missing: an unknown runtime is flagged at
/// validate, and a load failure shouldn't silently drop declared servers.
fn runtime_supports_mcp(compose: &Compose, h: AgentHandle<'_>) -> bool {
    let runtimes = crate::runtimes::load_all(&compose.root).unwrap_or_default();
    runtimes
        .get(h.spec.runtime.as_str())
        .map(|r| r.supports_mcp)
        .unwrap_or(true)
}

fn render_mcp(compose: &Compose, h: AgentHandle<'_>, team_mcp_bin: &str) -> String {
    let mut v = serde_json::json!({
        "mcpServers": {
            "team": {
                "command": team_mcp_bin,
                "args": team_server_args(compose, h),
                "env": {}
            }
        }
    });

    // #383 Phase 4: merge per-agent declared MCP servers alongside the
    // built-in `team` server. Unlike hooks (claude-only), MCP is the
    // runtime-agnostic bus, so declared servers render for every runtime
    // whose descriptor sets `supports_mcp`. The `team` server is the
    // mailbox transport: it stays unconditional and non-clobberable — a
    // declared server named `team` is skipped here (and rejected at
    // validate) so it can never shadow the bus. env values pass through
    // verbatim; the runtime performs any `${VAR}` expansion.
    if !h.spec.mcps.is_empty() {
        if runtime_supports_mcp(compose, h) {
            let servers = v["mcpServers"]
                .as_object_mut()
                .expect("mcpServers is a json object");
            for (name, server) in &h.spec.mcps {
                if name == "team" {
                    continue; // non-clobberable bus; validate rejects this too
                }
                servers.insert(
                    name.clone(),
                    serde_json::to_value(server).expect("serialize McpServer"),
                );
            }
        } else {
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} MCP server(s) but runtime `{}` does not set `supports_mcp`; ignoring",
                h.project,
                h.agent,
                h.spec.mcps.len(),
                h.spec.runtime
            );
        }
    }

    serde_json::to_string_pretty(&v).expect("json")
}

/// Per-agent Codex `config.toml` for `runtime: codex` agents. Returns
/// `None` for every other runtime. Codex has no `--mcp-config` flag —
/// MCP servers are read from `[mcp_servers.<name>]` tables in
/// `$CODEX_HOME/config.toml` — so this file is the codex-shaped mirror of
/// [`render_mcp`]'s JSON: the unconditional `team` bus plus declared
/// `mcps:` under the same `supports_mcp` gate (render_mcp already warns
/// when the gate drops them, so this stays quiet). TOML is hand-rendered
/// with proper string escaping — team-core carries no toml crate and one
/// table shape doesn't earn the dependency.
pub fn render_codex_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> Option<String> {
    if h.spec.runtime != "codex" {
        return None;
    }
    let mut s = String::from(
        "# teamctl-managed: rewritten on every `teamctl up` / `reload`.\n\
         # Customize MCP servers through the compose file's `mcps:` field.\n",
    );
    push_mcp_server_table(
        &mut s,
        "team",
        team_mcp_bin,
        &team_server_args(compose, h),
        &Default::default(),
    );
    if !h.spec.mcps.is_empty() && runtime_supports_mcp(compose, h) {
        for (name, server) in &h.spec.mcps {
            if name == "team" {
                continue; // non-clobberable bus; validate rejects this too
            }
            push_mcp_server_table(&mut s, name, &server.command, &server.args, &server.env);
        }
    }
    Some(s)
}

/// Write (or clear) the per-agent Codex `config.toml`. Mirrors
/// [`write_subagents_json`]: the scoped + full render paths both call it
/// so a `mcps:` edit flows into the agent at the next render. For
/// non-codex agents only the managed `config.toml` is removed — never the
/// home dir itself, which may hold codex session history worth keeping.
pub fn write_codex_config(
    compose: &Compose,
    h: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> io::Result<()> {
    let dest = codex_home_dir(&compose.root, h.project, h.agent).join("config.toml");
    match render_codex_config(compose, h, team_mcp_bin) {
        Some(toml) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, toml)
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

/// Append one `[mcp_servers.<name>]` table (plus its `.env` sub-table
/// when non-empty) in the shape codex reads from `config.toml`.
fn push_mcp_server_table(
    buf: &mut String,
    name: &str,
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) {
    buf.push_str(&format!("\n[mcp_servers.{}]\n", toml_key(name)));
    buf.push_str(&format!("command = {}\n", toml_str(command)));
    let args: Vec<String> = args.iter().map(|a| toml_str(a)).collect();
    buf.push_str(&format!("args = [{}]\n", args.join(", ")));
    if !env.is_empty() {
        buf.push_str(&format!("\n[mcp_servers.{}.env]\n", toml_key(name)));
        for (k, v) in env {
            buf.push_str(&format!("{} = {}\n", toml_key(k), toml_str(v)));
        }
    }
}

/// Render a string as a TOML basic string (double-quoted). Backslash,
/// quote, and control characters are the only escapes basic strings
/// require; everything else passes through verbatim (values are not
/// interpolated, matching render's no-`${VAR}`-expansion rule).
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render a TOML key: bare when it fits TOML's bare-key charset, quoted
/// otherwise (server names and env keys come from operator YAML and
/// aren't constrained to bare-safe characters).
fn toml_key(k: &str) -> String {
    let bare = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        k.to_string()
    } else {
        toml_str(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn fixture() -> Compose {
        let mut managers = BTreeMap::new();
        managers.insert(
            "mgr".into(),
            Agent {
                runtime: "claude-code".into(),
                model: Some("claude-opus-4-8".into()),
                role_prompt: Some(RolePrompt::Single(PathBuf::from("roles/mgr.md"))),
                permission_mode: Some("auto".into()),
                autonomy: "low_risk_only".into(),
                can_dm: vec![],
                can_broadcast: vec![],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
                ultracode: false,
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
                skills: vec![],
            },
        );
        Compose {
            root: PathBuf::from("/teamctl"),
            global: Global {
                version: crate::compose::SchemaVersion::new("2.0.0"),
                broker: Broker {
                    r#type: "sqlite".into(),
                    path: PathBuf::from("state/mailbox.db"),
                },
                supervisor: SupervisorCfg {
                    r#type: "tmux".into(),
                    tmux_prefix: "a-".into(),
                    drain_timeout_secs: 10,
                },
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
                attachments: Default::default(),
            },
            projects: vec![Project {
                version: 2,
                project: ProjectMeta {
                    id: "hello".into(),
                    name: "Hello".into(),
                    cwd: PathBuf::from("/teamctl/examples/hello-team"),
                },
                channels: vec![],
                managers,
                workers: Default::default(),
                interfaces: None,
            }],
        }
    }

    #[test]
    fn env_contains_agent_id_and_mailbox() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(env.contains("AGENT_ID=hello:mgr"));
        assert!(env.contains("TEAMCTL_MAILBOX=/teamctl/state/mailbox.db"));
        assert!(env.contains("TMUX_SESSION=a-hello-mgr"));
    }

    #[test]
    fn env_emits_claude_session_id_and_name_for_claude_code_runtime() {
        // T-118: claude-code agents get deterministic UUIDv5 session
        // ids in their env so the wrapper can pass `--session-id` +
        // `-n` and resume the conversation across restarts.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let expected_id = crate::session::derive_session_id(h.project, h.agent);
        assert!(
            env.contains(&format!("CLAUDE_SESSION_ID={expected_id}\n")),
            "env was: {env}"
        );
        assert!(
            env.contains("CLAUDE_SESSION_NAME=teamctl:hello:mgr\n"),
            "env was: {env}"
        );
    }

    #[test]
    fn env_omits_claude_session_vars_for_non_claude_runtimes() {
        // Other runtimes (codex, gemini) don't recognize claude's
        // `--session-id` flag — their wrapper arms must not see these
        // vars. Pin the gate so a future render refactor can't leak
        // them into every runtime.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            !env.contains("CLAUDE_SESSION_ID="),
            "non-claude runtime must not get session id: {env}"
        );
        assert!(
            !env.contains("CLAUDE_SESSION_NAME="),
            "non-claude runtime must not get session name: {env}"
        );
    }

    #[test]
    fn env_pins_teamctl_root_to_compose_root() {
        // Regression: when project.cwd is a relative path (e.g. `..`),
        // the wrapper used to fall back to it for `--root`, which
        // resolves against the post-cd cwd and points at the wrong
        // directory. Rendering an absolute TEAMCTL_ROOT pins
        // `teamctl --root` to the compose root regardless of cwd.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(env.contains("TEAMCTL_ROOT=/teamctl\n"), "env was: {env}");
    }

    #[test]
    fn env_omits_effort_when_unset() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(!env.contains("EFFORT="), "env was: {env}");
    }

    #[test]
    fn env_emits_effort_when_set() {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().effort = Some(EffortLevel::Max);
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(env.contains("EFFORT=max\n"), "env was: {env}");
    }

    #[test]
    fn mcp_json_parses_back() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        assert_eq!(
            v["mcpServers"]["team"]["command"],
            "/usr/local/bin/team-mcp"
        );
        assert_eq!(
            v["mcpServers"]["team"]["args"][1].as_str().unwrap(),
            "hello:mgr"
        );
    }

    #[test]
    fn mcp_json_threads_tmux_prefix_from_compose() {
        // T-109: compact_self routes its tmux send-keys to
        // `<prefix><project>-<agent>` and reads the prefix from a CLI arg
        // (default `t-` only fits a stock team). Render must surface the
        // configured prefix so teams overriding it (e.g. `a-` here) get
        // their pane resolved correctly.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        let args: Vec<&str> = v["mcpServers"]["team"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let i = args.iter().position(|a| *a == "--tmux-prefix").expect(
            "render_mcp must emit --tmux-prefix so compact_self resolves the caller's pane",
        );
        assert_eq!(
            args[i + 1],
            "a-",
            "prefix must come from compose, not the default"
        );
    }

    /// Build a `McpServer` test value tersely.
    fn server(command: &str, args: &[&str]) -> McpServer {
        McpServer {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
        }
    }

    #[test]
    fn mcp_json_includes_declared_servers_alongside_team() {
        // #383 Phase 4: a declared server lands in `mcpServers` next to
        // the built-in `team` server, with command/args/env passed
        // through verbatim (no `${VAR}` expansion in render).
        let mut c = fixture();
        let mut mcps = BTreeMap::new();
        let mut gh = server("npx", &["-y", "@modelcontextprotocol/server-github"]);
        gh.env
            .insert("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into());
        mcps.insert("github".into(), gh);
        c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;

        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();

        // Built-in team server survives untouched.
        assert_eq!(
            v["mcpServers"]["team"]["command"],
            "/usr/local/bin/team-mcp"
        );
        // Declared server present with verbatim fields.
        assert_eq!(v["mcpServers"]["github"]["command"], "npx");
        assert_eq!(v["mcpServers"]["github"]["args"][0], "-y");
        assert_eq!(
            v["mcpServers"]["github"]["env"]["GITHUB_TOKEN"], "${GITHUB_TOKEN}",
            "env values must pass through verbatim — the runtime expands ${{VAR}}"
        );
        assert_eq!(v["mcpServers"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn mcp_json_team_server_is_non_clobberable() {
        // #383 Phase 4: a declared server literally named `team` must not
        // shadow the built-in mailbox bus — render skips it (validate also
        // rejects it). The `team` entry keeps the built-in command.
        let mut c = fixture();
        let mut mcps = BTreeMap::new();
        mcps.insert("team".into(), server("evil-team", &[]));
        mcps.insert("github".into(), server("npx", &[]));
        c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;

        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();

        assert_eq!(
            v["mcpServers"]["team"]["command"], "/usr/local/bin/team-mcp",
            "built-in team server must not be clobbered by a declared `team`"
        );
        assert!(v["mcpServers"]["github"].is_object());
        assert_eq!(
            v["mcpServers"].as_object().unwrap().len(),
            2,
            "the declared `team` is dropped, not added as a third entry"
        );
    }

    #[test]
    fn mcp_json_unchanged_when_no_servers_declared() {
        // #383 Phase 4: empty `mcps` (the default) → only the built-in
        // team server, exactly as before this feature.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        let servers = v["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("team"));
    }

    #[test]
    fn mcp_json_skips_declared_servers_on_runtime_without_mcp_support() {
        // #383 Phase 4: declared servers render only for runtimes whose
        // descriptor sets `supports_mcp`. A custom runtime that opts out
        // gets the team bus (unconditional) but not the declared servers.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("runtimes")).unwrap();
        std::fs::write(
            tmp.path().join("runtimes/codex.yaml"),
            "binary: codex\nsupports_mcp: false\n",
        )
        .unwrap();

        let mut c = fixture();
        c.root = tmp.path().to_path_buf();
        {
            let m = c.projects[0].managers.get_mut("mgr").unwrap();
            m.runtime = "codex".into();
            let mut mcps = BTreeMap::new();
            mcps.insert("github".into(), server("npx", &[]));
            m.mcps = mcps;
        }

        let h = c.agents().next().unwrap();
        let (_, mcp) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        let servers = v["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("team"), "team bus stays unconditional");
        assert!(
            !servers.contains_key("github"),
            "declared server skipped when runtime lacks supports_mcp"
        );
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn env_emits_codex_home_for_codex_runtime() {
        // Codex has no --mcp-config flag; the wrapper's codex arm reads
        // CODEX_HOME from the env file and exports it so codex finds the
        // rendered [mcp_servers.*] config.toml.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            env.contains("CODEX_HOME=/teamctl/state/codex-home/hello-mgr\n"),
            "env was: {env}"
        );
    }

    #[test]
    fn env_omits_codex_home_for_non_codex_runtimes() {
        // Only the codex arm consumes CODEX_HOME; leaking it into other
        // runtimes' envs would relocate their state if a same-named knob
        // ever appears. Pin the gate for claude-code and gemini both.
        for runtime in ["claude-code", "gemini"] {
            let mut c = fixture();
            c.projects[0].managers.get_mut("mgr").unwrap().runtime = runtime.into();
            let h = c.agents().next().unwrap();
            let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
            assert!(
                !env.contains("CODEX_HOME="),
                "{runtime} must not get CODEX_HOME: {env}"
            );
        }
    }

    #[test]
    fn codex_config_present_with_team_server_for_codex_runtime() {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp")
            .expect("codex agent must get a config.toml");
        assert!(toml.contains("[mcp_servers.team]"), "toml was: {toml}");
        assert!(
            toml.contains("command = \"/usr/local/bin/team-mcp\""),
            "toml was: {toml}"
        );
        // Same args as the JSON transport — the shared team_server_args
        // helper is the single source of truth.
        assert!(
            toml.contains("\"--agent-id\", \"hello:mgr\""),
            "toml was: {toml}"
        );
        assert!(
            toml.contains("\"--tmux-prefix\", \"a-\""),
            "toml was: {toml}"
        );
    }

    #[test]
    fn codex_config_absent_for_non_codex_runtimes() {
        // claude/gemini get their MCP servers via the JSON file; a stray
        // config.toml would be dead weight on disk.
        let c = fixture();
        let h = c.agents().next().unwrap();
        assert!(render_codex_config(&c, h, "/usr/local/bin/team-mcp").is_none());
    }

    #[test]
    fn codex_config_includes_declared_servers() {
        // Declared `mcps:` land as their own [mcp_servers.<name>] tables
        // next to the team bus, mirroring the JSON transport's merge.
        let mut c = fixture();
        {
            let m = c.projects[0].managers.get_mut("mgr").unwrap();
            m.runtime = "codex".into();
            let mut gh = server("npx", &["-y", "@modelcontextprotocol/server-github"]);
            gh.env
                .insert("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into());
            let mut mcps = BTreeMap::new();
            mcps.insert("github".into(), gh);
            m.mcps = mcps;
        }
        let h = c.agents().next().unwrap();
        let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
        assert!(toml.contains("[mcp_servers.team]"), "toml was: {toml}");
        assert!(toml.contains("[mcp_servers.github]"), "toml was: {toml}");
        assert!(toml.contains("command = \"npx\""), "toml was: {toml}");
        assert!(
            toml.contains("args = [\"-y\", \"@modelcontextprotocol/server-github\"]"),
            "toml was: {toml}"
        );
        assert!(
            toml.contains("[mcp_servers.github.env]\nGITHUB_TOKEN = \"${GITHUB_TOKEN}\""),
            "env must land in a sub-table, verbatim: {toml}"
        );
    }

    #[test]
    fn codex_config_escapes_toml_strings() {
        // A quote or backslash in an env value must not break the TOML —
        // basic strings escape both (team-core hand-renders, no toml crate).
        let mut c = fixture();
        {
            let m = c.projects[0].managers.get_mut("mgr").unwrap();
            m.runtime = "codex".into();
            let mut srv = server("run", &[]);
            srv.env
                .insert("TRICKY".into(), "say \"hi\" C:\\path".into());
            let mut mcps = BTreeMap::new();
            mcps.insert("x".into(), srv);
            m.mcps = mcps;
        }
        let h = c.agents().next().unwrap();
        let toml = render_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
        assert!(
            toml.contains("TRICKY = \"say \\\"hi\\\" C:\\\\path\""),
            "toml was: {toml}"
        );
    }

    #[test]
    fn write_codex_config_writes_then_clears_stale() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = fixture();
        c.root = dir.path().to_path_buf();
        let dest = codex_home_dir(&c.root, "hello", "mgr").join("config.toml");

        // codex runtime → config materialized under the per-agent home.
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        write_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
        assert!(dest.exists(), "codex config.toml should be written");

        // Runtime switched away → the managed config is removed (the home
        // dir itself survives: it may hold codex session history).
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "claude-code".into();
        let h = c.agents().next().unwrap();
        write_codex_config(&c, h, "/usr/local/bin/team-mcp").unwrap();
        assert!(!dest.exists(), "stale codex config.toml should be removed");
    }

    #[test]
    fn env_points_at_source_for_single_role_prompt() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            env.contains("SYSTEM_PROMPT_PATH=/teamctl/roles/mgr.md\n"),
            "env was: {env}"
        );
    }

    #[test]
    fn env_points_at_concat_path_for_multi_role_prompt() {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
                PathBuf::from("roles/_base.md"),
                PathBuf::from("roles/mgr.md"),
            ]));
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            env.contains("SYSTEM_PROMPT_PATH=/teamctl/state/role_prompts/hello-mgr.md\n"),
            "env was: {env}"
        );
    }

    #[test]
    fn write_role_prompt_concat_is_noop_for_single() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = fixture();
        c.root = dir.path().to_path_buf();
        let h = c.agents().next().unwrap();
        write_role_prompt_concat(&c, h).unwrap();
        assert!(
            !role_prompt_concat_path(&c.root, h.project, h.agent).exists(),
            "single-form role_prompt should not produce a concat file"
        );
    }

    #[test]
    fn write_role_prompt_concat_joins_in_declared_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        std::fs::write(root.join("roles/_base.md"), "BASE").unwrap();
        std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

        let mut c = fixture();
        c.root = root.to_path_buf();
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
                PathBuf::from("roles/_base.md"),
                PathBuf::from("roles/mgr.md"),
            ]));
        let h = c.agents().next().unwrap();
        write_role_prompt_concat(&c, h).unwrap();

        let dest = role_prompt_concat_path(root, h.project, h.agent);
        let got = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(got, "BASE\n\n—\n\nMGR");
    }

    #[test]
    fn write_role_prompt_concat_reflects_source_edits() {
        // Owner-flagged: editing a source file must show up at the next
        // render. We re-write unconditionally rather than caching.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("roles")).unwrap();
        std::fs::write(root.join("roles/_base.md"), "v1").unwrap();
        std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

        let mut c = fixture();
        c.root = root.to_path_buf();
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(RolePrompt::Multiple(vec![
                PathBuf::from("roles/_base.md"),
                PathBuf::from("roles/mgr.md"),
            ]));
        let h = c.agents().next().unwrap();
        write_role_prompt_concat(&c, h).unwrap();

        std::fs::write(root.join("roles/_base.md"), "v2").unwrap();
        let h = c.agents().next().unwrap();
        write_role_prompt_concat(&c, h).unwrap();

        let dest = role_prompt_concat_path(root, h.project, h.agent);
        let got = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(got, "v2\n\n—\n\nMGR");
    }

    #[test]
    fn claude_settings_present_for_claude_code() {
        // T-189: claude-code agents get a wrapper-managed settings
        // file with a PreToolUse deny hook for synchronous-prompt
        // tools that would otherwise strand a headless pane.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let pre = &v["hooks"]["PreToolUse"][0];
        assert_eq!(
            pre["matcher"].as_str().unwrap(),
            "AskUserQuestion|EnterPlanMode|ExitPlanMode"
        );
        let cmd = pre["hooks"][0]["command"].as_str().unwrap();
        assert!(
            cmd.contains(r#""permissionDecision":"deny""#),
            "deny verdict missing from hook command: {cmd}"
        );
        assert!(
            cmd.contains("Interactive prompts are disabled"),
            "systemMessage missing from hook command: {cmd}"
        );
    }

    #[test]
    fn claude_settings_pre_trust_all_project_mcp_servers() {
        // #421: the rendered settings carry `enableAllProjectMcpServers: true`
        // at the top level so a headless agent never freezes on Claude's "New
        // MCP server found in this project" prompt (no human to confirm it).
        // Attended sessions skip `--settings` entirely, so this only affects
        // unattended panes; non-claude runtimes get no settings file at all
        // (covered by `claude_settings_absent_for_non_claude_runtimes`).
        let c = fixture();
        let h = c.agents().next().unwrap();
        let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v["enableAllProjectMcpServers"],
            serde_json::Value::Bool(true),
            "headless settings must pre-trust project MCP servers: {s}"
        );
    }

    #[test]
    fn claude_settings_absent_for_non_claude_runtimes() {
        // codex/gemini don't read claude settings; the file would be
        // dead weight and a confusing artifact on disk.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        assert!(render_claude_settings(&c, h).is_none());
    }

    #[test]
    fn claude_settings_absent_when_non_claude_agent_opts_into_ultracode() {
        // #461: a declared `ultracode: true` on a non-claude runtime is a
        // no-op — the whole settings file is skipped, so the opt-in must
        // never leak into a rendered artifact. Pins that the early-return
        // None survives even with the opt-in set (the warn fires; the
        // contract is the None).
        let mut c = fixture();
        {
            let m = c.projects[0].managers.get_mut("mgr").unwrap();
            m.runtime = "codex".into();
            m.ultracode = true;
        }
        let h = c.agents().next().unwrap();
        assert!(render_claude_settings(&c, h).is_none());
    }

    #[test]
    fn declared_hook_merges_alongside_deny_hook() {
        // #383 Phase 2 + #428: a per-agent PreToolUse hook is appended
        // AFTER the built-ins in the same bucket — the deny hook keeps slot
        // 0, the #428 heartbeat touch sits at slot 1, and the declared hook
        // lands at slot 2 with its command resolved to an absolute path.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().hooks = vec![HookSpec {
            event: "PreToolUse".into(),
            matcher: Some("Bash".into()),
            command: PathBuf::from("hooks/guard.sh"),
        }];
        let h = c.agents().next().unwrap();
        let s = render_claude_settings(&c, h).expect("claude-code agent must get settings");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(
            pre.len(),
            3,
            "deny hook + #428 heartbeat touch + declared hook expected"
        );
        // Built-in deny hook survives in slot 0.
        assert_eq!(
            pre[0]["matcher"].as_str().unwrap(),
            "AskUserQuestion|EnterPlanMode|ExitPlanMode"
        );
        assert!(pre[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(r#""permissionDecision":"deny""#));
        // #428 heartbeat touch at slot 1 (match-all, no matcher).
        assert!(
            pre[1].get("matcher").is_none(),
            "heartbeat touch must be match-all: {}",
            pre[1]
        );
        // Declared hook appended after the built-ins.
        assert_eq!(pre[2]["matcher"].as_str().unwrap(), "Bash");
        assert_eq!(pre[2]["hooks"][0]["type"].as_str().unwrap(), "command");
        assert_eq!(
            pre[2]["hooks"][0]["command"].as_str().unwrap(),
            "/teamctl/hooks/guard.sh"
        );
    }

    #[test]
    fn claude_settings_emits_ultracode_when_set() {
        // #461: opting an agent into ultracode renders `"ultracode": true`
        // into its Claude Code settings JSON — the file the wrapper passes
        // via `--settings`.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().ultracode = true;
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        assert_eq!(v["ultracode"], true);
    }

    #[test]
    fn claude_settings_omits_ultracode_when_unset() {
        // #461: with ultracode left at its default (`false`), the key is
        // absent entirely — not present-and-false — so the settings shape is
        // byte-identical for agents that don't opt in.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        assert!(v.get("ultracode").is_none());
    }

    #[test]
    fn default_hooks_are_deny_plus_heartbeat_buckets() {
        // #383 Phase 2 + #428 + #430 + #431: with no compose-declared hooks,
        // the settings file renders exactly the built-in default buckets: the
        // `PreToolUse` deny hook, the #428 activity-heartbeat hooks, the #430
        // `SessionStart` boot-context hook, and the #431 `StopFailure`
        // rate-limit marker, and nothing else. Asserted as an exact key-set
        // (not a raw count) so each future built-in extends the set
        // deterministically instead of racing on a number.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let hooks = v["hooks"].as_object().unwrap();
        let keys: std::collections::BTreeSet<&str> = hooks.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "PreToolUse",
                "SessionStart",
                "Stop",
                "StopFailure",
                "UserPromptSubmit"
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
            "exact set of built-in default hook buckets expected with no declared hooks"
        );
        // PreToolUse holds the deny hook (slot 0) + the heartbeat touch.
        assert_eq!(
            hooks["PreToolUse"].as_array().unwrap().len(),
            2,
            "deny hook + heartbeat touch expected"
        );
        // StopFailure holds the heartbeat clear (slot 0) + the #431 rate-limit
        // marker (slot 1): slot 0 is the match-all `rm -f`, slot 1 carries the
        // `rate_limit` matcher and the `rl-hit` command.
        let stop_failure = hooks["StopFailure"].as_array().unwrap();
        assert_eq!(
            stop_failure.len(),
            2,
            "heartbeat clear + rate-limit marker expected"
        );
        assert!(
            stop_failure[0].get("matcher").is_none(),
            "StopFailure slot 0 should be the match-all heartbeat clear"
        );
        // #439: the heartbeat clear now records LASTSEEN before removing the
        // marker, so the command leads with `touch ` and still rm's the marker.
        let clear_cmd = stop_failure[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(
            clear_cmd.starts_with("touch ") && clear_cmd.contains(" && rm -f "),
            "StopFailure slot 0 should touch lastseen then rm the marker: {clear_cmd}"
        );
        assert_eq!(
            stop_failure[1]["matcher"].as_str().unwrap(),
            "rate_limit",
            "StopFailure slot 1 should scope to rate-limit stops"
        );
        assert!(stop_failure[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("rl-hit"));
        // Stop holds the heartbeat clear (slot 0) + the #333 budget cost writer
        // (slot 1): both are match-all (no matcher), slot 1 carries the
        // `budget-record` command.
        let stop = hooks["Stop"].as_array().unwrap();
        assert_eq!(
            stop.len(),
            2,
            "heartbeat clear + budget cost writer expected"
        );
        assert!(
            stop[0].get("matcher").is_none(),
            "Stop slot 0 should be the match-all heartbeat clear"
        );
        let stop_clear = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(
            stop_clear.starts_with("touch ") && stop_clear.contains(" && rm -f "),
            "Stop slot 0 should touch lastseen then rm the marker: {stop_clear}"
        );
        assert!(
            stop[1].get("matcher").is_none(),
            "Stop slot 1 (budget writer) should be match-all"
        );
        assert!(stop[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("budget-record"));
        // Each remaining single-entry built-in bucket holds exactly its one entry.
        for ev in ["UserPromptSubmit", "SessionStart"] {
            assert_eq!(
                hooks[ev].as_array().unwrap().len(),
                1,
                "{ev} should hold exactly one built-in entry"
            );
        }
    }

    #[test]
    fn stop_failure_rate_limit_hook_records_a_hit() {
        // #431: the StopFailure bucket's slot-1 entry is the rate-limit marker.
        // The canary `default_hooks_are_deny_plus_heartbeat_buckets` pins the
        // bucket shape (2 entries, slot 1 matcher `rate_limit` + `rl-hit`); this
        // pins the load-bearing details of the emitted command string.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let stop_failure = v["hooks"]["StopFailure"].as_array().unwrap();
        let command = stop_failure[1]["hooks"][0]["command"].as_str().unwrap();

        // Guard first so a host without `teamctl` on PATH never errors the stop.
        assert!(
            command.starts_with("command -v teamctl >/dev/null"),
            "rl-hit command must lead with the PATH guard: {command}"
        );
        // The compose root is baked in so the hook needs no env to find the db.
        assert!(
            command.contains("--root"),
            "rl-hit command must pass the compose --root: {command}"
        );
        // The subcommand and the agent's `<project>:<agent>` id, pulled from the
        // fixture handle so the assertion tracks the fixture rather than a
        // hard-coded literal.
        assert!(
            command.contains("rl-hit"),
            "rl-hit subcommand missing: {command}"
        );
        let agent_id = format!("{}:{}", h.project, h.agent);
        assert!(
            command.contains(&agent_id),
            "rl-hit command must target the agent id {agent_id}: {command}"
        );
        // Trailing `|| true` makes the marker pure fire-and-forget: any rl-hit
        // error degrades to a silent exit-0 instead of erroring the stop.
        assert!(
            command.ends_with("|| true"),
            "rl-hit command must end with the fire-and-forget guard: {command}"
        );
    }

    #[test]
    fn stop_budget_record_hook_records_cost() {
        // #333: the Stop bucket's slot-1 entry is the budget cost writer. The
        // canary `default_hooks_are_deny_plus_heartbeat_buckets` pins the bucket
        // shape (2 entries, slot 1 match-all + `budget-record`); this pins the
        // load-bearing details of the emitted command string, mirroring the #431
        // rl-hit canary exactly.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        let command = stop[1]["hooks"][0]["command"].as_str().unwrap();

        // Guard first so a host without `teamctl` on PATH never errors the stop.
        assert!(
            command.starts_with("command -v teamctl >/dev/null"),
            "budget-record command must lead with the PATH guard: {command}"
        );
        // The compose root is baked in so the hook needs no env to find the db.
        assert!(
            command.contains("--root"),
            "budget-record command must pass the compose --root: {command}"
        );
        // The subcommand and the agent's `<project>:<agent>` id, pulled from the
        // fixture handle so the assertion tracks the fixture rather than a
        // hard-coded literal.
        assert!(
            command.contains("budget-record"),
            "budget-record subcommand missing: {command}"
        );
        let agent_id = format!("{}:{}", h.project, h.agent);
        assert!(
            command.contains(&agent_id),
            "budget-record command must target the agent id {agent_id}: {command}"
        );
        // Trailing `|| true` makes the writer pure fire-and-forget: any record
        // error degrades to a silent exit-0 instead of erroring the stop.
        assert!(
            command.ends_with("|| true"),
            "budget-record command must end with the fire-and-forget guard: {command}"
        );
    }

    #[test]
    fn heartbeat_hooks_touch_and_clear_the_marker() {
        // #428: the four activity-heartbeat hooks render with the agent's
        // marker path, `type:command`, and no `matcher` (match-all). touch
        // on PreToolUse/UserPromptSubmit, rm on Stop/StopFailure.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let path = heartbeat_path(&c.root, h.project, h.agent)
            .display()
            .to_string();
        // Same shlex quoting the renderer uses — pins the exact emitted
        // command, so a regression in quoting (or a dropped `touch`/`rm`)
        // fails here rather than silently misfiring at runtime.
        let q = crate::supervisor::shlex::try_quote(&path).unwrap();
        // #439: the turn-end clear also touches the LASTSEEN sibling.
        let ls_path = lastseen_path(&c.root, h.project, h.agent)
            .display()
            .to_string();
        let ls = crate::supervisor::shlex::try_quote(&ls_path).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let hooks = &v["hooks"];

        // PreToolUse: deny stays at slot 0, heartbeat touch appended at slot 1.
        let touch_entry = &hooks["PreToolUse"].as_array().unwrap()[1];
        assert!(
            touch_entry.get("matcher").is_none(),
            "heartbeat must be match-all (no matcher): {touch_entry}"
        );
        assert_eq!(touch_entry["hooks"][0]["type"].as_str().unwrap(), "command");
        assert_eq!(
            touch_entry["hooks"][0]["command"].as_str().unwrap(),
            format!("touch {q}"),
            "PreToolUse should touch the quoted marker"
        );

        // UserPromptSubmit touches the same marker.
        assert_eq!(
            hooks["UserPromptSubmit"].as_array().unwrap()[0]["hooks"][0]["command"]
                .as_str()
                .unwrap(),
            format!("touch {q}"),
            "UserPromptSubmit should touch the quoted marker"
        );

        // Stop + StopFailure clear it. The heartbeat clear is always slot 0
        // (match-all); on StopFailure the #431 rate-limit marker follows at
        // slot 1, so the heartbeat entry keeps its slot. #439: the clear first
        // touches the LASTSEEN sibling, then rm's the marker, in one command.
        for ev in ["Stop", "StopFailure"] {
            let entry = &hooks[ev].as_array().unwrap()[0];
            assert!(
                entry.get("matcher").is_none(),
                "{ev} must be match-all (no matcher)"
            );
            assert_eq!(
                entry["hooks"][0]["command"].as_str().unwrap(),
                format!("touch {ls} && rm -f {q}"),
                "{ev} should touch the quoted lastseen then rm the quoted marker"
            );
        }
    }

    #[test]
    fn session_start_hook_runs_the_boot_script() {
        // #430: the SessionStart boot-context hook renders as a single
        // match-all entry whose command is the shlex-quoted absolute path to
        // the shared `bin/boot.sh` asset, with a 5s timeout. #439: the command
        // now passes two positional argv — the quoted LASTSEEN then MARKER
        // paths — so the script can compute downtime. The script (not this
        // JSON) emits the REQUIRED `hookEventName` — here we pin the wiring
        // that points Claude Code at it, that it carries the per-agent argv,
        // and that it fires on every source (no matcher).
        let c = fixture();
        let h = c.agents().next().unwrap();
        let path = boot_script_path(&c.root).display().to_string();
        let q = crate::supervisor::shlex::try_quote(&path).unwrap();
        let ls_path = lastseen_path(&c.root, h.project, h.agent)
            .display()
            .to_string();
        let ls = crate::supervisor::shlex::try_quote(&ls_path).unwrap();
        let marker_path = heartbeat_path(&c.root, h.project, h.agent)
            .display()
            .to_string();
        let marker = crate::supervisor::shlex::try_quote(&marker_path).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        let bucket = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            bucket.len(),
            1,
            "exactly one SessionStart built-in expected"
        );
        let entry = &bucket[0];
        assert!(
            entry.get("matcher").is_none(),
            "SessionStart must be match-all (fire on every source): {entry}"
        );
        let inner = &entry["hooks"][0];
        assert_eq!(inner["type"].as_str().unwrap(), "command");
        assert_eq!(
            inner["command"].as_str().unwrap(),
            format!("{q} {ls} {marker}"),
            "SessionStart should run the quoted boot.sh path with lastseen + marker argv"
        );
        assert_eq!(inner["timeout"].as_i64().unwrap(), 5, "5s timeout expected");
    }

    #[test]
    fn lastseen_path_is_a_dotlastseen_sibling_of_the_marker() {
        // #439: the lastseen marker lives in the same state/heartbeats dir as
        // the heartbeat marker, with the same `<project>-<agent>` stem plus a
        // `.lastseen` suffix — so it never shadows the marker the TUI stats for
        // Working/Idle. The two render tests use this fn on both sides of their
        // assertions; this pins the literal shape they rely on.
        let root = std::path::Path::new("/srv/.team");
        let marker = heartbeat_path(root, "proj", "ada");
        let lastseen = lastseen_path(root, "proj", "ada");
        assert_eq!(lastseen, root.join("state/heartbeats/proj-ada.lastseen"));
        assert_eq!(
            lastseen.parent(),
            marker.parent(),
            "lastseen must sit in the same heartbeats dir as the marker"
        );
        assert_ne!(
            lastseen, marker,
            "lastseen must not collide with the marker"
        );
    }

    #[test]
    fn declared_hook_without_matcher_opens_new_event_bucket() {
        // #383 Phase 2: a hook on a fresh event (no matcher) creates its
        // own bucket and omits `matcher` so Claude Code matches all tools;
        // PreToolUse keeps only its built-ins (deny + #428 heartbeat) since
        // this declared hook targets PostToolUse.
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().hooks = vec![HookSpec {
            event: "PostToolUse".into(),
            matcher: None,
            command: PathBuf::from("hooks/log.sh"),
        }];
        let h = c.agents().next().unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&render_claude_settings(&c, h).unwrap()).unwrap();
        assert_eq!(
            v["hooks"]["PreToolUse"].as_array().unwrap().len(),
            2,
            "PreToolUse keeps its deny + #428 heartbeat built-ins"
        );
        let post = &v["hooks"]["PostToolUse"].as_array().unwrap()[0];
        assert!(
            post.get("matcher").is_none(),
            "matcher must be omitted when unset: {post}"
        );
        assert_eq!(
            post["hooks"][0]["command"].as_str().unwrap(),
            "/teamctl/hooks/log.sh"
        );
    }

    #[test]
    fn declared_hooks_noop_on_non_claude_runtime() {
        // #383 Phase 2: hooks are claude-only v1 — declared on codex the
        // whole settings file is still skipped (render warns, returns None).
        let mut c = fixture();
        {
            let m = c.projects[0].managers.get_mut("mgr").unwrap();
            m.runtime = "codex".into();
            m.hooks = vec![HookSpec {
                event: "PreToolUse".into(),
                matcher: Some("Bash".into()),
                command: PathBuf::from("hooks/guard.sh"),
            }];
        }
        let h = c.agents().next().unwrap();
        assert!(
            render_claude_settings(&c, h).is_none(),
            "hooks must not render on non-claude runtimes"
        );
    }

    #[test]
    fn env_emits_claude_settings_path_for_claude_code() {
        // T-189: wrapper reads CLAUDE_SETTINGS and passes it to claude
        // via `--settings`. Path must resolve under the compose root.
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            env.contains("CLAUDE_SETTINGS=/teamctl/state/claude/hello-mgr.json\n"),
            "env was: {env}"
        );
    }

    #[test]
    fn env_omits_claude_settings_for_non_claude_runtimes() {
        // Only claude-code reads the settings file; other runtimes
        // must not see the env var (avoids confusion if they ever add
        // a same-named knob).
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            !env.contains("CLAUDE_SETTINGS="),
            "non-claude runtime must not get settings path: {env}"
        );
    }

    #[test]
    fn write_role_prompt_concat_errors_on_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = fixture();
        c.root = dir.path().to_path_buf();
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(RolePrompt::Multiple(
            vec![PathBuf::from("roles/missing.md")],
        ));
        let h = c.agents().next().unwrap();
        let err = write_role_prompt_concat(&c, h).unwrap_err();
        assert!(err.to_string().contains("missing.md"), "err was: {err}");
    }

    // ---- #383 Phase 3a: per-agent sub-agents (`--agents` JSON) ----

    fn write_file(root: &std::path::Path, rel: &str, contents: &str) {
        let abs = root.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, contents).unwrap();
    }

    fn rooted(write: impl FnOnce(&std::path::Path)) -> (tempfile::TempDir, Compose) {
        let dir = tempfile::tempdir().unwrap();
        let mut c = fixture();
        c.root = dir.path().to_path_buf();
        write(dir.path());
        (dir, c)
    }

    #[test]
    fn render_subagents_builds_agents_json_from_frontmatter() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/security-auditor.md",
                "---\nname: security-auditor\ndescription: Audits diffs for vulns.\n\
                 tools: Read, Grep\nmodel: claude-sonnet-4-6\n---\n\
                 You are a security auditor.\nFlag risky patterns.\n",
            );
        });
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/security-auditor.md")];
        let h = c.agents().next().unwrap();
        let json = render_subagents(&c, h).unwrap().expect("some json");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &v["security-auditor"];
        assert_eq!(entry["description"], "Audits diffs for vulns.");
        assert_eq!(
            entry["prompt"],
            "You are a security auditor.\nFlag risky patterns."
        );
        assert_eq!(entry["tools"], serde_json::json!(["Read", "Grep"]));
        assert_eq!(entry["model"], "claude-sonnet-4-6");
    }

    #[test]
    fn render_subagents_name_falls_back_to_file_stem() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/repo-cartographer.md",
                "---\ndescription: Maps the repo.\n---\nMap it.\n",
            );
        });
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/repo-cartographer.md")];
        let h = c.agents().next().unwrap();
        let json = render_subagents(&c, h).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("repo-cartographer").is_some(),
            "stem-derived name missing: {json}"
        );
        // Nothing declared beyond description → optional keys omitted.
        assert!(v["repo-cartographer"].get("tools").is_none());
        assert!(v["repo-cartographer"].get("model").is_none());
    }

    #[test]
    fn render_subagents_supports_yaml_list_tools() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/x.md",
                "---\nname: x\ndescription: d\ntools: [Read, Bash]\n---\nbody\n",
            );
        });
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/x.md")];
        let h = c.agents().next().unwrap();
        let json = render_subagents(&c, h).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["x"]["tools"], serde_json::json!(["Read", "Bash"]));
    }

    #[test]
    fn render_subagents_isolates_per_agent() {
        // Two agents declaring different sub-agents must each get only
        // their own — the core per-agent-scope guarantee.
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/a.md",
                "---\nname: a\ndescription: da\n---\nba\n",
            );
            write_file(
                root,
                "agents/b.md",
                "---\nname: b\ndescription: db\n---\nbb\n",
            );
        });
        let worker = c.projects[0].managers["mgr"].clone();
        c.projects[0].workers.insert("dev".into(), worker);
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/a.md")];
        c.projects[0].workers.get_mut("dev").unwrap().subagents =
            vec![PathBuf::from("agents/b.md")];

        for h in c.agents() {
            let v: serde_json::Value =
                serde_json::from_str(&render_subagents(&c, h).unwrap().unwrap()).unwrap();
            match h.agent {
                "mgr" => {
                    assert!(v.get("a").is_some() && v.get("b").is_none());
                }
                "dev" => {
                    assert!(v.get("b").is_some() && v.get("a").is_none());
                }
                other => panic!("unexpected agent {other}"),
            }
        }
    }

    #[test]
    fn render_subagents_none_when_empty() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        assert!(render_subagents(&c, h).unwrap().is_none());
    }

    #[test]
    fn render_subagents_ignored_on_non_claude_runtime() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/x.md",
                "---\nname: x\ndescription: d\n---\nb\n",
            );
        });
        {
            let a = c.projects[0].managers.get_mut("mgr").unwrap();
            a.runtime = "codex".into();
            a.subagents = vec![PathBuf::from("agents/x.md")];
        }
        let h = c.agents().next().unwrap();
        // claude-only v1: codex ignores declared sub-agents (warns).
        assert!(render_subagents(&c, h).unwrap().is_none());
    }

    #[test]
    fn render_subagents_errors_on_missing_source() {
        let (_d, mut c) = rooted(|_| {});
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/nope.md")];
        let h = c.agents().next().unwrap();
        let err = render_subagents(&c, h).unwrap_err();
        assert!(err.to_string().contains("nope.md"), "err was: {err}");
    }

    #[test]
    fn render_subagents_errors_on_unterminated_frontmatter() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/bad.md",
                "---\nname: x\ndescription: d\nno close\n",
            );
        });
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/bad.md")];
        let h = c.agents().next().unwrap();
        assert!(render_subagents(&c, h).is_err());
    }

    #[test]
    fn env_emits_claude_agents_json_for_claude_code() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(env.contains("CLAUDE_AGENTS_JSON=/teamctl/state/claude/hello-mgr.agents.json"));
    }

    #[test]
    fn write_subagents_json_writes_then_clears_stale() {
        let (_d, mut c) = rooted(|root| {
            write_file(
                root,
                "agents/x.md",
                "---\nname: x\ndescription: d\n---\nbody\n",
            );
        });
        let dest = subagents_json_path(&c.root, "hello", "mgr");

        // Declared → file materialized.
        c.projects[0].managers.get_mut("mgr").unwrap().subagents =
            vec![PathBuf::from("agents/x.md")];
        let h = c.agents().next().unwrap();
        write_subagents_json(&c, h).unwrap();
        assert!(dest.exists(), "agents json should be written");

        // Dropped → stale file removed so old sub-agents don't linger.
        c.projects[0].managers.get_mut("mgr").unwrap().subagents = vec![];
        let h = c.agents().next().unwrap();
        write_subagents_json(&c, h).unwrap();
        assert!(!dest.exists(), "stale agents json should be removed");
    }

    #[test]
    fn write_agent_skills_materializes_symlinks() {
        let (_d, mut c) = rooted(|root| {
            write_file(root, "skills/pr-review/SKILL.md", "# PR review skill\n");
        });
        c.projects[0].managers.get_mut("mgr").unwrap().skills =
            vec![PathBuf::from("skills/pr-review")];
        let h = c.agents().next().unwrap();
        write_agent_skills(&c, h).unwrap();

        let link = agent_scope_dir(&c.root, "hello", "mgr").join(".claude/skills/pr-review");
        let meta = std::fs::symlink_metadata(&link).expect("link should exist");
        assert!(meta.file_type().is_symlink(), "entry must be a symlink");
        // Resolves to the source skill dir (so CC finds its SKILL.md).
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(c.root.join("skills/pr-review")).unwrap()
        );
    }

    #[test]
    fn write_agent_skills_clear_stale_preserves_source() {
        // SAFETY: dropping a skill must unlink only the symlink — never
        // recurse into and delete the real skill directory it pointed at.
        let (_d, mut c) = rooted(|root| {
            write_file(root, "skills/foo/SKILL.md", "# foo\n");
        });
        let source = c.root.join("skills/foo");
        let source_md = source.join("SKILL.md");

        // Declare → materialize the link.
        c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![PathBuf::from("skills/foo")];
        let h = c.agents().next().unwrap();
        write_agent_skills(&c, h).unwrap();
        let scope = agent_scope_dir(&c.root, "hello", "mgr");
        assert!(scope.join(".claude/skills/foo").exists());

        // Drop → scope cleared, but the real skill dir + SKILL.md survive.
        c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![];
        let h = c.agents().next().unwrap();
        write_agent_skills(&c, h).unwrap();
        assert!(!scope.exists(), "stale scope dir should be removed");
        assert!(source.is_dir(), "source skill dir must survive the clear");
        assert!(
            source_md.is_file(),
            "source SKILL.md must survive the clear"
        );
    }

    #[test]
    fn write_agent_skills_isolates_per_agent() {
        // Two agents declaring different skills must each get only their
        // own — the core per-agent-scope guarantee.
        let (_d, mut c) = rooted(|root| {
            write_file(root, "skills/a/SKILL.md", "# a\n");
            write_file(root, "skills/b/SKILL.md", "# b\n");
        });
        let worker = c.projects[0].managers["mgr"].clone();
        c.projects[0].workers.insert("dev".into(), worker);
        c.projects[0].managers.get_mut("mgr").unwrap().skills = vec![PathBuf::from("skills/a")];
        c.projects[0].workers.get_mut("dev").unwrap().skills = vec![PathBuf::from("skills/b")];

        for h in c.agents() {
            write_agent_skills(&c, h).unwrap();
        }
        let mgr_skills = agent_scope_dir(&c.root, "hello", "mgr").join(".claude/skills");
        let dev_skills = agent_scope_dir(&c.root, "hello", "dev").join(".claude/skills");
        assert!(mgr_skills.join("a").exists() && !mgr_skills.join("b").exists());
        assert!(dev_skills.join("b").exists() && !dev_skills.join("a").exists());
    }

    #[test]
    fn write_agent_skills_ignored_on_non_claude_runtime() {
        let (_d, mut c) = rooted(|root| {
            write_file(root, "skills/x/SKILL.md", "# x\n");
        });
        {
            let a = c.projects[0].managers.get_mut("mgr").unwrap();
            a.runtime = "codex".into();
            a.skills = vec![PathBuf::from("skills/x")];
        }
        let h = c.agents().next().unwrap();
        // claude-only v1: codex ignores declared skills (warns) and no
        // scope dir is created.
        write_agent_skills(&c, h).unwrap();
        assert!(!agent_scope_dir(&c.root, "hello", "mgr").exists());
    }

    #[test]
    fn env_emits_claude_agent_scope_for_claude_code() {
        let c = fixture();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(env.contains("CLAUDE_AGENT_SCOPE=/teamctl/state/agent-scope/hello-mgr"));
    }

    #[test]
    fn env_omits_claude_agent_scope_for_non_claude_runtimes() {
        let mut c = fixture();
        c.projects[0].managers.get_mut("mgr").unwrap().runtime = "codex".into();
        let h = c.agents().next().unwrap();
        let (env, _) = render_agent(&c, h, "/usr/local/bin/team-mcp");
        assert!(
            !env.contains("CLAUDE_AGENT_SCOPE="),
            "non-claude runtime must not get the agent scope: {env}"
        );
    }
}
