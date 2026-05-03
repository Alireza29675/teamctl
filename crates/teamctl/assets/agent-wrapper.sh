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
: "${BOOTSTRAP_PROMPT:=Begin your shift as ${AGENT}. Team traffic is delivered to you as \`<channel source=\"team\">\` events via Claude Code Channels -- you do not need to poll. Process each event per your role and the system prompt, calling \`inbox_ack\` on the message ids you handle. Between events, idle. Use \`inbox_peek\` only for catch-up after a restart.}"

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
}

# Claude Code shows a non-persistent confirmation dialog every time it
# starts with `--dangerously-load-development-channels` for a server
# that isn't on Anthropic's allowlist. team-mcp is off-allowlist during
# the Channels research preview, so the dialog reappears on every
# wrapper restart and strands the agent at "Press Enter to confirm".
#
# This watcher polls our own tmux pane for the dialog header and sends
# one Enter when it appears, then exits. Bounded at 60s so it never
# lingers past a genuine prompt the operator is answering. No-op once
# team-mcp is allowlisted (the header never shows up), and no-op
# outside tmux (TMUX_PANE unset).
auto_confirm_dev_channels() {
    pane="${TMUX_PANE:-${TMUX_SESSION:-}}"
    [ -z "$pane" ] && return 0
    command -v tmux >/dev/null 2>&1 || return 0
    end=$(( $(date +%s) + 60 ))
    while [ "$(date +%s)" -lt "$end" ]; do
        if tmux capture-pane -t "$pane" -p 2>/dev/null \
            | grep -qF "Loading development channels"; then
            tmux send-keys -t "$pane" Enter
            return 0
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
        auto_confirm_dev_channels &
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
