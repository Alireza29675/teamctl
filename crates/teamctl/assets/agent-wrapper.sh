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
: "${BOOTSTRAP_PROMPT:=Begin your shift as ${AGENT}. Open inbox_watch via the \`team\` MCP server and keep it open. Process any messages per your role and the system prompt. Stay running -- do not exit.}"

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
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
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
                set -- "$@" --append-system-prompt "$(cat "$SYSTEM_PROMPT_PATH")"
            set -- "$@" "$BOOTSTRAP_PROMPT"
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

    if command -v teamctl >/dev/null 2>&1; then
        teamctl --root "$TEAMCTL_ROOT" rl-watch "$AGENT" -- "$BIN" "$@"
    else
        log "teamctl not on PATH — running runtime directly (no rate-limit handling)"
        "$BIN" "$@"
    fi
    ec=$?
    log "runtime exited ec=$ec — restarting in 5s"
    sleep 5
done
