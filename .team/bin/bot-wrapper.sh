#!/bin/sh
# teamctl bot wrapper.
#
# Invoked by the tmux session `teamctl up` creates for each declared
# `interfaces.telegram` (ADR 0005). Responsibilities:
#   - source the workspace .env so the namespaced secrets are visible
#   - translate `TEAMCTL_TELEGRAM_<INFIX>_TOKEN` / `_CHATS` into the
#     un-namespaced names team-bot's CLI reads
#   - loop on team-bot so a transient crash auto-restarts
#
# This file is teamctl-managed: `teamctl up` rewrites it every run.
# Customize behaviour through env vars in `state/envs/bot-<id>.env`,
# not by editing the script.
#
# First positional arg is the bot id (matches `interfaces.telegram.id`).

set -u

BOT="${1:-${BOT_ID:-}}"
if [ -z "$BOT" ]; then
    echo "bot-wrapper: bot id not provided (arg or \$BOT_ID)" >&2
    exit 2
fi

: "${BOT_INFIX:=}"
: "${MANAGER:=}"
: "${TEAMCTL_MAILBOX:=}"
: "${TEAMCTL_ROOT:=.}"

log() {
    printf '[bot-wrapper %s] %s\n' "$BOT" "$*" >&2
}

if [ -z "$BOT_INFIX" ]; then
    BOT_INFIX=$(printf '%s' "$BOT" | tr 'a-z-' 'A-Z_')
fi

# Source the workspace .env so the namespaced secret is visible. We check
# the .team-relative location first (the supervisor's $TEAMCTL_ROOT is the
# .team directory or the flat layout root) and fall back to the parent for
# `.team/.env` deployments where users put `.env` next to the repo.
for candidate in "$TEAMCTL_ROOT/.env" "$TEAMCTL_ROOT/../.env"; do
    if [ -f "$candidate" ]; then
        log "sourcing $candidate"
        set -a
        # shellcheck disable=SC1090
        . "$candidate"
        set +a
        break
    fi
done

# Map namespaced -> un-namespaced. team-bot's clap reads $TEAMCTL_TELEGRAM_TOKEN
# and $TEAMCTL_TELEGRAM_CHATS (plus the new $TEAMCTL_BOT_ID / $TEAMCTL_MANAGER),
# which keeps the binary itself unaware of the per-id namespace scheme.
token_var="TEAMCTL_TELEGRAM_${BOT_INFIX}_TOKEN"
chats_var="TEAMCTL_TELEGRAM_${BOT_INFIX}_CHATS"
eval "TEAMCTL_TELEGRAM_TOKEN=\${$token_var:-}"
eval "TEAMCTL_TELEGRAM_CHATS=\${$chats_var:-}"
export TEAMCTL_TELEGRAM_TOKEN TEAMCTL_TELEGRAM_CHATS

if [ -z "$TEAMCTL_TELEGRAM_TOKEN" ]; then
    log "$token_var not set in $TEAMCTL_ROOT/.env — run 'teamctl bot setup $BOT'"
    # Sleep so the tmux pane stays alive long enough for `teamctl logs` /
    # `teamctl bot status` to surface the message instead of returning
    # "stopped" with no context.
    sleep 30
    exit 2
fi

export TEAMCTL_BOT_ID="$BOT"
export TEAMCTL_MANAGER="$MANAGER"
export TEAMCTL_MAILBOX

while :; do
    log "starting team-bot bot=$BOT manager=$MANAGER"
    if command -v team-bot >/dev/null 2>&1; then
        team-bot
    else
        log "team-bot not on PATH"
        sleep 30
        continue
    fi
    ec=$?
    log "team-bot exited ec=$ec — restarting in 5s"
    sleep 5
done
