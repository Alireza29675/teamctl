#!/bin/sh
# teamctl agent wrapper.
#
# Invoked by the tmux session `teamctl up` creates. Responsible for:
#   - sourcing the per-agent env file (via the tmux command's `env`)
#   - looping on the runtime so crashes auto-restart
#   - routing every runtime invocation through `teamctl rl-watch` so
#     rate-limits get parsed, hooks fire, and we sleep until the limit
#     window has cleared before respawning.
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

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
}

# Build the runtime invocation as positional args. The wrapper hands the
# whole thing to `teamctl rl-watch -- …`, which spawns the runtime under
# a parsing pipeline. If teamctl is not on PATH, we fall back to direct
# exec — at the cost of dumb retries on rate-limit hits.
build_claude_args() {
    set --
    [ -n "$PERMISSION_MODE" ] && set -- "$@" --permission-mode "$PERMISSION_MODE"
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
        set -- "$@" --append-system-prompt "$(cat "$SYSTEM_PROMPT_PATH")"
    BIN=claude
    BIN_ARGS="$*"
}

build_codex_args() {
    set --
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --instructions "$SYSTEM_PROMPT_PATH"
    BIN=codex
    BIN_ARGS="$*"
}

build_gemini_args() {
    set --
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --system-instruction-file "$SYSTEM_PROMPT_PATH"
    set -- "$@" --yolo
    BIN=gemini
    BIN_ARGS="$*"
}

while :; do
    log "starting runtime=$RUNTIME model=${MODEL:-<default>}"
    case "$RUNTIME" in
        claude-code) build_claude_args ;;
        codex)       build_codex_args ;;
        gemini)      build_gemini_args ;;
        *)
            log "unknown runtime: $RUNTIME"
            sleep 30
            continue
            ;;
    esac

    if command -v teamctl >/dev/null 2>&1; then
        # shellcheck disable=SC2086
        teamctl --root "$TEAMCTL_ROOT" rl-watch "$AGENT" -- "$BIN" $BIN_ARGS
    else
        log "teamctl not on PATH — running runtime directly (no rate-limit handling)"
        # shellcheck disable=SC2086
        "$BIN" $BIN_ARGS
    fi
    ec=$?
    log "runtime exited ec=$ec — restarting in 5s"
    sleep 5
done
