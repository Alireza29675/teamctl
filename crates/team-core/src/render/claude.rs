use super::*;

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
