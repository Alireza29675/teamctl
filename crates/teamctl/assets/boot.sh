#!/bin/sh
# teamctl boot-context hook (#430).
#
# Wired into Claude Code's `SessionStart` hook by `render_claude_settings`.
# It injects a one-line wake notice into the agent's context so a freshly
# (re)started session knows it just woke and from which transition. This
# supersedes the bootstrap-prompt mechanism #258 sketched, keeping its
# "tell the agent it was down" idea on a real hook event.
#
# Claude Code delivers the SessionStart payload as JSON on stdin; its
# `source` field is one of startup|resume|clear|compact. We read it with a
# POSIX sed extraction rather than jq, which is not guaranteed on the host.
#
# CRITICAL: the emitted JSON MUST carry `hookEventName` inside
# `hookSpecificOutput`. Without it Claude Code silently drops
# `additionalContext` -- the hook appears to run but injects nothing.
#
# This file is teamctl-managed: `teamctl up` rewrites it on every run.

set -u

# CC's SessionStart payload has exactly one top-level scalar `source`
# (startup|resume|clear|compact), so the greedy match is unambiguous. The
# `[^"]*` capture stops at the first quote, so `src` can never contain a `"`
# and is therefore JSON-safe to interpolate into the output below.
payload=$(cat)
src=$(printf '%s' "$payload" \
    | sed -n 's/.*"source"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n 1)
[ -n "$src" ] || src=startup

case "$src" in
    resume)  verb="resumed" ;;
    clear)   verb="cleared context" ;;
    compact) verb="compacted" ;;
    *)       verb="booted" ;;
esac

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)

printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"You %s at %s (source: %s)."}}\n' \
    "$verb" "$now" "$src"
