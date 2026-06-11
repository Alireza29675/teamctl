#!/bin/sh
# teamctl boot-context hook (#430, #439).
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
# #439 adds two source-specific extensions to the base notice:
#   * on `startup`, a coarse downtime sentence computed from the agent's
#     last-activity file mtime (argv $1 = LASTSEEN, $2 = MARKER, below);
#   * on `compact`, a re-anchor paragraph reminding the agent to re-read its
#     working files since unwritten context was just trimmed.
# Both per-agent paths arrive as argv (the #428 per-agent precedent), so this
# script stays shared and agent-agnostic. Under `set -u` they are optional:
# an older rendered hook passes no argv, in which case downtime is omitted.
#
# CRITICAL: the emitted JSON MUST carry `hookEventName` inside
# `hookSpecificOutput`. Without it Claude Code silently drops
# `additionalContext` -- the hook appears to run but injects nothing.
#
# Every injected string is ASCII with no double quote, backslash, or
# apostrophe, so the single-quoted `printf` below hand-builds valid JSON
# without any escaping.
#
# This file is teamctl-managed: `teamctl up` rewrites it on every run.

set -u

# Per-agent paths from render (#439); absent on an older rendered hook.
lastseen=${1:-}
marker=${2:-}

# Epoch mtime of a file, portably across GNU coreutils and BSD/macOS.
# GNU's own flag (`-c %Y`) is tried FIRST: BSD stat rejects `-c` and exits
# non-zero, so the fallback to BSD `-f %m` only fires on macOS. The reverse
# order would misfire -- BSD's `-f` means --file-system on GNU and prints
# garbage with exit 0, so the `||` would never trigger. The numeric guard
# drops any non-integer (unknown stat flavor, missing file) so a bad read
# omits the sentence rather than emitting nonsense.
file_mtime() {
    m=$(stat -c %Y "$1" 2>/dev/null) || m=$(stat -f %m "$1" 2>/dev/null) || return 1
    case "$m" in
        '' | *[!0-9]*) return 1 ;;
    esac
    printf '%s' "$m"
}

# Coarse downtime bucket for N seconds (N >= 60). ASCII, singular-aware.
# under 60m -> "N minutes"; under 24h -> "H hours[ M minutes]"; else
# "D days[ H hours]" (the smaller unit dropped when zero).
format_duration() {
    secs=$1
    if [ "$secs" -lt 3600 ]; then
        mins=$((secs / 60))
        if [ "$mins" -eq 1 ]; then printf '1 minute'; else printf '%s minutes' "$mins"; fi
        return
    fi
    if [ "$secs" -lt 86400 ]; then
        hours=$((secs / 3600))
        mins=$(((secs % 3600) / 60))
        if [ "$hours" -eq 1 ]; then h='1 hour'; else h="$hours hours"; fi
        if [ "$mins" -eq 0 ]; then printf '%s' "$h"
        elif [ "$mins" -eq 1 ]; then printf '%s 1 minute' "$h"
        else printf '%s %s minutes' "$h" "$mins"; fi
        return
    fi
    days=$((secs / 86400))
    hours=$(((secs % 86400) / 3600))
    if [ "$days" -eq 1 ]; then d='1 day'; else d="$days days"; fi
    if [ "$hours" -eq 0 ]; then printf '%s' "$d"
    elif [ "$hours" -eq 1 ]; then printf '%s 1 hour' "$d"
    else printf '%s %s hours' "$d" "$hours"; fi
}

# Downtime sentence for `startup`, or empty (return 1) if not computable.
# Prefers the MARKER mtime when the marker still exists -- its presence at
# boot means an unclean mid-turn stop, so its mtime is the last real activity
# and beats the clean-turn-end LASTSEEN. Omits cleanly on first boot, missing
# files, an unreadable mtime, or a non-positive diff (clock skew).
downtime_sentence() {
    last=
    if [ -n "$marker" ] && [ -e "$marker" ]; then
        last=$(file_mtime "$marker") || last=
    fi
    if [ -z "$last" ] && [ -n "$lastseen" ] && [ -e "$lastseen" ]; then
        last=$(file_mtime "$lastseen") || last=
    fi
    [ -n "$last" ] || return 1

    nowsec=$(date -u +%s)
    case "$nowsec" in
        '' | *[!0-9]*) return 1 ;;
    esac
    diff=$((nowsec - last))
    [ "$diff" -gt 0 ] || return 1

    # Epoch -> ISO-8601 UTC. BSD's own flag (`-r SECONDS`) is tried FIRST:
    # GNU date reads `-r` as a FILE and exits non-zero on the numeric epoch,
    # so the fallback to GNU `-d @SECONDS` only fires on Linux. BSD-first
    # rather than GNU-first because some BSD `date` builds read `-d` as the
    # daylight-saving flag instead of erroring, which would print the current
    # time with exit 0 and the `||` would never trigger; trying the native
    # `-r` first sidesteps that on every BSD.
    iso=$(date -u -r "$last" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null) \
        || iso=$(date -u -d "@$last" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null) \
        || return 1

    if [ "$diff" -lt 60 ]; then
        printf 'You were down for under a minute (last active %s).' "$iso"
    else
        printf 'You were down for about %s (last active %s).' "$(format_duration "$diff")" "$iso"
    fi
}

# CC's SessionStart payload has exactly one top-level scalar `source`
# (startup|resume|clear|compact), so the greedy match is unambiguous. The
# `[^"]*` capture stops at the first quote, but on its own still admits a
# backslash -- so the `case` below normalizes `src` to the known source set.
# The value interpolated into the JSON output is therefore always one of four
# fixed ASCII literals: JSON-safe by construction, not by trusting the payload.
payload=$(cat)
src=$(printf '%s' "$payload" \
    | sed -n 's/.*"source"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n 1)

# Map source -> past-tense verb, normalizing any value outside the known set
# (empty, missing, or exotic) to a fresh `startup` so it is both treated as a
# boot and whitelisted before it can reach the JSON output.
case "$src" in
    resume)  verb="resumed" ;;
    clear)   verb="cleared context" ;;
    compact) verb="compacted" ;;
    startup) verb="booted" ;;
    *)       verb="booted"; src=startup ;;
esac

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
context="You $verb at $now (source: $src)."

# Source-specific extensions (#439). startup: append the downtime sentence
# when computable. compact: append the re-anchor reminder (no file deps).
# resume / clear are deliberately left as the base notice -- resume is a live
# session continuing, not downtime.
case "$src" in
    startup)
        extra=$(downtime_sentence) && [ -n "$extra" ] && context="$context $extra"
        ;;
    compact)
        context="$context Prior conversation detail was just summarized and trimmed. Re-anchor before continuing: re-read your working files (task list, working memory, notes) and resume from what is written there - do not assume unwritten context survived."
        ;;
esac

printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' \
    "$context"
