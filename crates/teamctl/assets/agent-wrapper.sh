#!/bin/sh
# teamctl agent wrapper.
#
# Invoked by the tmux session `teamctl up` creates. Responsible for:
#   - sourcing the per-agent env file
#   - looping on the runtime binary so crashes auto-restart
#
# First positional arg is `<project>:<agent>`. The env file has already been
# loaded by `env $(cat <envfile>) …` in the tmux command, so we just need to
# dispatch on $RUNTIME.

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

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
}

run_claude_code() {
    set --
    [ -n "$PERMISSION_MODE" ] && set -- "$@" --permission-mode "$PERMISSION_MODE"
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
        set -- "$@" --append-system-prompt "$(cat "$SYSTEM_PROMPT_PATH")"
    exec claude "$@"
}

run_codex() {
    set --
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --instructions "$SYSTEM_PROMPT_PATH"
    exec codex "$@"
}

run_gemini() {
    set --
    [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
    [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
    [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --system-instruction-file "$SYSTEM_PROMPT_PATH"
    exec gemini --yolo "$@"
}

while :; do
    log "starting runtime=$RUNTIME model=${MODEL:-<default>}"
    case "$RUNTIME" in
        claude-code) run_claude_code ;;
        codex)       run_codex ;;
        gemini)      run_gemini ;;
        *)
            log "unknown runtime: $RUNTIME"
            sleep 30
            ;;
    esac
    ec=$?
    log "runtime exited ec=$ec — restarting in 5s"
    sleep 5
done
