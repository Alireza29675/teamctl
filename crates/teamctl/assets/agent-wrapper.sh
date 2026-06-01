#!/bin/sh
# teamctl agent wrapper.
#
# Invoked by the tmux session `teamctl up` creates. Responsible for:
#   - sourcing the per-agent env file (via the tmux command's `env`)
#   - looping on the runtime so crashes auto-restart
#   - routing every runtime invocation through `teamctl rl-watch` so
#     the runtime gets a real pty (interactive REPL), rate-limit
#     signatures get parsed, hooks fire, and we sleep until the limit
#     window has cleared before respawning.
#
# This file is teamctl-managed: `teamctl up` rewrites it on every run.
# Customize behaviour through env vars (BOOTSTRAP_PROMPT, MODEL, ...)
# rather than editing the script.
#
# First positional arg is `<project>:<agent>`.

set -u

AGENT="${1:-${AGENT_ID:-}}"
if [ -z "$AGENT" ]; then
    echo "agent-wrapper: AGENT id not provided (arg or \$AGENT_ID)" >&2
    exit 2
fi

: "${RUNTIME:=claude-code}"
: "${MODEL:=}"
: "${PERMISSION_MODE:=}"
: "${CLAUDE_SETTINGS:=}"
: "${MCP_CONFIG:=}"
: "${CLAUDE_AGENTS_JSON:=}"
: "${CLAUDE_AGENT_SCOPE:=}"
: "${SYSTEM_PROMPT_PATH:=}"
: "${CLAUDE_PROJECT_DIR:=.}"
: "${TEAMCTL_ROOT:=$CLAUDE_PROJECT_DIR}"
# Rendered into the env file only when the YAML `effort:` field is set.
# Default to empty here so `set -u` doesn't trip the `[ -n "$EFFORT" ]`
# check below for agents that omit it.
: "${EFFORT:=}"
# T-118 / T-174: rendered into the env file only for claude-code
# runtime. Default to empty under `set -u` so the wrapper's
# `[ -n "$CLAUDE_SESSION_ID" ]` and `[ -n "$CLAUDE_SESSION_NAME" ]`
# checks below are safe even when these vars are absent from the env
# file (env file written by an older teamctl render, missing-var
# write race, non-claude-code runtime co-existing in the same
# wrapper). Without this default, `set -u` aborts the wrapper at the
# unguarded reference, the tmux pane closes immediately, and the
# supervisor marks the agent stopped without ever printing a
# diagnostic.
: "${CLAUDE_SESSION_ID:=}"
: "${CLAUDE_SESSION_NAME:=}"
# T-190: macOS ships bash 3.2 as `/bin/sh`. Bash 3.2 has a parser
# bug in `${VAR:=DEFAULT}` parameter-expansion: it cannot reliably
# parse escape sequences inside the DEFAULT (backslash-backtick,
# backslash-quote). This wrapper's `BOOTSTRAP_PROMPT` default
# contains both — `\`<channel source=\"team\">\`` and friends — so
# every spawn on macOS aborts at this line with "unexpected EOF
# while looking for matching `}`", the tmux pane closes, and the
# supervisor marks the agent stopped. Linux dash + bash 4+ parse
# the construct correctly, which is why this regression hid through
# Linux qa.
#
# Fix: pull the default OUT of `${VAR:=...}` form into a regular
# conditional assignment. A double-quoted string literal parses
# identically on bash 3.2 / 4+ / dash, so the escapes work everywhere.
# Behavior is unchanged when BOOTSTRAP_PROMPT is already set by the
# env file (the `[ -z ]` short-circuits).
if [ -z "${BOOTSTRAP_PROMPT:-}" ]; then
    BOOTSTRAP_PROMPT="Begin your shift as ${AGENT}. Team traffic is delivered to you as \`<channel source=\"team\">\` events via Claude Code Channels -- you do not need to poll. By default the body is a short \"📬 1 new message ...\" stub (meta.lazy=\"1\"); call \`inbox_read\` with the meta.id to fetch the full body and resolve it in one step. If the stub doesn't merit handling, call \`inbox_ack\` to dismiss. When the body lands inline (no meta.lazy, e.g. operator used \`/readnow\`), act on it directly and call \`inbox_ack\` on the id when done. Between events, idle. Use \`inbox_peek\` only for non-destructive catch-up after a restart."
fi

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
}

# Claude Code surfaces a handful of one-shot confirmation dialogs that
# strand a headless agent because no operator is at the keyboard:
#
#   - "Loading development channels"   — fires every wrapper start
#     while team-mcp is off Anthropic's allowlist (Channels research
#     preview).
#   - "Bypass Permissions mode"        — fires on first launch under
#     `permission_mode: bypassPermissions` (the opt-in escape hatch)
#     when the acceptance marker isn't on disk (fresh $HOME, new VM).
#   - "Stop and wait for limit to reset" — fires when claude hits a
#     usage-limit cap and asks the operator whether to wait, switch
#     to extra usage, or upgrade. Default-highlighted option is
#     "wait", which is the right choice for a supervised headless
#     agent (operator can intervene manually for a different choice).
#
# The watcher polls our own tmux pane for any of these headers and
# sends one Enter when matched, then sleeps 1s so the dialog clears
# from the captured frame before the next poll (otherwise the same
# match would re-fire). Patterns are anchored on text that only
# appears inside these specific dialogs, so accidental matches
# against legitimate operator prompts are unlikely.
#
# The watcher runs for the full lifetime of the runtime (the limit
# prompt can fire at any point, not only at boot) and is reaped by
# the outer loop after the runtime exits. No-op outside tmux
# (TMUX_PANE unset).
auto_confirm_known_dialogs() {
    pane="${TMUX_PANE:-${TMUX_SESSION:-}}"
    [ -z "$pane" ] && return 0
    command -v tmux >/dev/null 2>&1 || return 0
    while :; do
        if tmux capture-pane -t "$pane" -p 2>/dev/null \
            | grep -qE 'Loading development channels|Bypass Permissions mode|Stop and wait for limit to reset'; then
            tmux send-keys -t "$pane" Enter
            sleep 1
            continue
        fi
        sleep 0.5
    done
}

# Build the runtime invocation as the script's positional parameters.
# Doing this in-line (instead of in a function) keeps the args quoted —
# previous versions stuffed everything into a single $BIN_ARGS string and
# re-split on whitespace, which silently corrupted multi-word values like
# the role prompt.
while :; do
    log "starting runtime=$RUNTIME model=${MODEL:-<default>}"
    case "$RUNTIME" in
        claude-code)
            BIN=claude
            set --
            # T-118: deterministic session id + display name so the
            # conversation persists across teamctl down/up + crash
            # recovery. UUIDv5 is rendered into the env file by
            # team-core (claude-code-only); claude creates the session
            # at this UUID on first spawn and resumes it on every
            # subsequent spawn. If the session-file at this UUID is
            # ever removed (manual cleanup, claude session-dir reset),
            # claude creates a fresh one at the same UUID — self-
            # healing by construction. The BOOTSTRAP_PROMPT keeps
            # being injected on every spawn (option 1 from #118):
            # claude's session-storage slug rule isn't a documented
            # contract, so a cold-vs-warm probe here would couple the
            # wrapper to a behavior that could shift between claude
            # versions. The bootstrap is small enough that warm-start
            # cost is trivial, and the model recognizes "I've already
            # done this".
            #
            # T-174: claude code 2.1.138 split `--session-id` into
            # create-only semantics — passing it on a UUID whose jsonl
            # already exists errors with "Session ID is already in
            # use". To attach to an existing session you must use
            # `--resume <UUID>` instead. Probe the on-disk session
            # path; when it exists, splice `--resume`. On a fresh
            # spawn (or after manual cleanup) the path is absent and
            # we fall through to the original `--session-id` shape —
            # the deterministic UUID is unchanged, only the flag
            # selection branches. The `-n` display-name flag is kept
            # on both branches: claude tolerates it on `--resume`
            # (no-op when the existing session already carries the
            # name) and we don't want to bet the agent's tmux
            # identity on undocumented persistence.
            #
            # Glob (`projects/*/...`) on purpose — claude's
            # cwd-to-project-dir slug is observed-not-documented
            # (currently `/` and `.` → `-`, may shift). The UUIDv5 is
            # globally unique per agent, so at most one file ever
            # matches and we never have to mirror that algorithm.
            if [ -n "$CLAUDE_SESSION_ID" ]; then
                if ls "$HOME/.claude/projects/"*/"$CLAUDE_SESSION_ID.jsonl" >/dev/null 2>&1; then
                    set -- "$@" --resume "$CLAUDE_SESSION_ID"
                else
                    set -- "$@" --session-id "$CLAUDE_SESSION_ID"
                fi
            fi
            [ -n "$CLAUDE_SESSION_NAME" ] && set -- "$@" -n "$CLAUDE_SESSION_NAME"
            # T-189 / T-361: `permission_mode: attended` is the opt-out for
            # the headless-default footgun protections. When attended, a
            # human is at the keyboard and can answer interactive prompts,
            # so we skip both:
            #   - `--permission-mode` (claude has no "attended" mode — it's
            #     a teamctl-level concept; a human drives the normal prompts),
            #   - `--settings <hook-deny>` (let interactive tools run).
            # Any other permission_mode (or unset) means headless. We default
            # to `auto`: claude's classifier lets routine work run without
            # prompts and blocks risky actions outright, so an unattended pane
            # keeps draining its inbox instead of freezing on a permission
            # dialog. (Edge: if auto blocks 3x consecutively or 20x total in a
            # session it falls back to prompting — see CHANGELOG.) An operator
            # who genuinely needs the old bypass-everything behavior for a
            # disposable sandbox can set `permission_mode: bypassPermissions`,
            # which flows through here (no teamctl-specific escape hatch). We
            # also ship the deny hook so AskUserQuestion / plan-mode pickers
            # can't strand the pane.
            if [ "${PERMISSION_MODE:-}" = "attended" ]; then
                :
            else
                set -- "$@" --permission-mode "${PERMISSION_MODE:-auto}"
                [ -n "$CLAUDE_SETTINGS" ] && set -- "$@" --settings "$CLAUDE_SETTINGS"
            fi
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            # T-048: per-agent reasoning effort. Source order is YAML
            # (rendered into this env file) > workspace `.env` (env
            # inherited from the operator shell) > unset (claude's own
            # default). Empty string is treated as unset.
            [ -n "$EFFORT" ] && set -- "$@" --effort "$EFFORT"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            # #383 Phase 3a: per-agent sub-agents. render writes the
            # `--agents` JSON only when the agent declares `subagents:`, so
            # the `[ -f ]` guard means no flag is passed when the file is
            # absent. Command substitution passes the JSON as a single arg
            # (embedded newlines preserved, `$`/backticks not re-expanded).
            [ -n "$CLAUDE_AGENTS_JSON" ] && [ -f "$CLAUDE_AGENTS_JSON" ] && \
                set -- "$@" --agents "$(cat "$CLAUDE_AGENTS_JSON")"
            # #383 Phase 3b: per-agent skills. render materializes a scope
            # dir with symlinks to declared skills under
            # <scope>/.claude/skills/ only when the agent declares `skills:`,
            # so the `[ -d ]` guard means no flag is passed when the dir is
            # absent. `--add-dir` is variadic; it sits before the `--`
            # terminator below so the bootstrap prompt isn't slurped.
            [ -n "$CLAUDE_AGENT_SCOPE" ] && [ -d "$CLAUDE_AGENT_SCOPE" ] && \
                set -- "$@" --add-dir "$CLAUDE_AGENT_SCOPE"
            # Subscribe to the team mailbox via Claude Code Channels
            # (v2.1.80+). team-mcp emits `notifications/claude/channel`
            # for every new inbox row, which lands in this session as
            # a `<channel source="team">` event -- so the agent reacts
            # on arrival without polling and idles silently between
            # events. `server:team` references the `team` entry in the
            # MCP config rendered above.
            #
            # `--dangerously-load-development-channels` (not `--channels`)
            # is required while team-mcp is off Anthropic's allowlist
            # during the Channels research preview. `--channels` would
            # be silently dropped here.
            set -- "$@" --dangerously-load-development-channels server:team
            [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
                set -- "$@" --append-system-prompt "$(cat "$SYSTEM_PROMPT_PATH")"
            # `--` terminates the variadic dev-channels list so the bare
            # BOOTSTRAP_PROMPT positional isn't slurped as another channel
            # entry.
            set -- "$@" -- "$BOOTSTRAP_PROMPT"
            AUTO_CONFIRM=1
            ;;
        codex)
            BIN=codex
            set --
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --instructions "$SYSTEM_PROMPT_PATH"
            set -- "$@" "$BOOTSTRAP_PROMPT"
            ;;
        gemini)
            BIN=gemini
            set --
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --system-instruction-file "$SYSTEM_PROMPT_PATH"
            set -- "$@" --yolo "$BOOTSTRAP_PROMPT"
            ;;
        *)
            log "unknown runtime: $RUNTIME"
            sleep 30
            continue
            ;;
    esac

    AUTO_CONFIRM_PID=
    if [ "${AUTO_CONFIRM:-0}" = 1 ]; then
        auto_confirm_known_dialogs &
        AUTO_CONFIRM_PID=$!
    fi
    AUTO_CONFIRM=0

    if command -v teamctl >/dev/null 2>&1; then
        teamctl --root "$TEAMCTL_ROOT" rl-watch "$AGENT" -- "$BIN" "$@"
    else
        log "teamctl not on PATH — running runtime directly (no rate-limit handling)"
        "$BIN" "$@"
    fi
    ec=$?

    if [ -n "$AUTO_CONFIRM_PID" ]; then
        kill "$AUTO_CONFIRM_PID" 2>/dev/null
        wait "$AUTO_CONFIRM_PID" 2>/dev/null
    fi

    log "runtime exited ec=$ec — restarting in 5s"
    sleep 5
done
