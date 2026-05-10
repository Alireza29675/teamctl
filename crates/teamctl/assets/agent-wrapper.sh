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
: "${MCP_CONFIG:=}"
: "${SYSTEM_PROMPT_PATH:=}"
: "${CLAUDE_PROJECT_DIR:=.}"
: "${TEAMCTL_ROOT:=$CLAUDE_PROJECT_DIR}"
# Rendered into the env file only when the YAML `effort:` field is set.
# Default to empty here so `set -u` doesn't trip the `[ -n "$EFFORT" ]`
# check below for agents that omit it.
: "${EFFORT:=}"
: "${BOOTSTRAP_PROMPT:=Begin your shift as ${AGENT}. Team traffic is delivered to you as \`<channel source=\"team\">\` events via Claude Code Channels -- you do not need to poll. By default the body is a short \"📬 1 new message ...\" stub (meta.lazy=\"1\"); call \`inbox_read\` with the meta.id to fetch the full body and resolve it in one step. If the stub doesn't merit handling, call \`inbox_ack\` to dismiss. When the body lands inline (no meta.lazy, e.g. operator used \`/readnow\`), act on it directly and call \`inbox_ack\` on the id when done. Between events, idle. Use \`inbox_peek\` only for non-destructive catch-up after a restart.}"

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
#     --dangerously-skip-permissions when the acceptance marker isn't
#     on disk (fresh $HOME, fresh user, new VM).
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
            [ -n "$CLAUDE_SESSION_ID" ] && set -- "$@" --session-id "$CLAUDE_SESSION_ID"
            [ -n "$CLAUDE_SESSION_NAME" ] && set -- "$@" -n "$CLAUDE_SESSION_NAME"
            [ -n "$PERMISSION_MODE" ] && set -- "$@" --permission-mode "$PERMISSION_MODE"
            # Autonomous agents have no human at the keyboard, so any
            # permission prompt deadlocks the pane. Skip them at the
            # claude layer; teamctl's HITL gate (request_approval via
            # team-mcp + the agent's `autonomy:` field) is the proper
            # human-in-loop ring instead.
            set -- "$@" --dangerously-skip-permissions
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            # T-048: per-agent reasoning effort. Source order is YAML
            # (rendered into this env file) > workspace `.env` (env
            # inherited from the operator shell) > unset (claude's own
            # default). Empty string is treated as unset.
            [ -n "$EFFORT" ] && set -- "$@" --effort "$EFFORT"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
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
