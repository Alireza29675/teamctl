#!/usr/bin/env bash
# teamctl audit hook — appends every interaction (tool calls, incoming
# prompts, and incoming channel messages) to a per-agent JSONL trail,
# so the team keeps a complete, durable record of everything it did.
# This is the mechanical backbone of "stateless by design": the agent
# can self-compact freely because its full action history survives on
# disk, independent of its context window.
#
# Wired into .claude/settings.json for PostToolUse + UserPromptSubmit.
# Each teamctl agent runs as a Claude Code session in this repo, so the
# project-level hook applies to all of them.
#
# - PostToolUse captures every tool call: its input AND result. That
#   includes the agents' own actions, their sub-agent spawns (Task),
#   and incoming channel messages (the inbox_read result carries the
#   full message body).
# - UserPromptSubmit captures inbound prompts and the
#   `<channel source="team">` event stubs.
#
# Known limit: a sub-agent's RESULT is logged only if its Task returns
# before the parent compacts/restarts. Work dispatched-but-not-returned
# across a compact is not captured here — roles re-dispatch it on wake
# (see _base.md "On wake, recover what didn't return").
#
# Only logs for teamctl agents — AGENT_ID is set by the teamctl wrapper
# (e.g. "teamctl:hugo"). Any other Claude Code session (like a human
# running `claude` in this repo) has no AGENT_ID, so this no-ops.

set -u

[ -n "${AGENT_ID:-}" ] || exit 0          # not a teamctl agent → skip

payload="$(cat)"                           # hook event JSON on stdin
[ -n "$payload" ] || exit 0

agent="${AGENT_ID##*:}"                     # "teamctl:hugo" → "hugo"

root="${TEAMCTL_ROOT:-}"                    # absolute path to the .team dir
if [ -z "$root" ]; then
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  root="$here/../../.team"                  # repo/.claude/hooks → repo/.team
fi

audit_dir="$root/state/$agent/audit"
mkdir -p "$audit_dir" 2>/dev/null || exit 0

sid="$(printf '%s' "$payload" | jq -r '.session_id // "nosession"' 2>/dev/null)"
[ -n "$sid" ] || sid="nosession"
ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# One file per session keeps concurrent sub-agents from interleaving a
# line. The agent's full trail is every *.jsonl under its audit/ dir.
logfile="$audit_dir/${sid}.jsonl"

# Append the whole event, tagged with timestamp + agent.
if ! printf '%s' "$payload" \
     | jq -c --arg ts "$ts" --arg agent "$agent" '{ts:$ts, agent:$agent} + .' \
     >> "$logfile" 2>/dev/null; then
  # jq couldn't parse it (shouldn't happen) — keep the raw line so the
  # trail never silently drops an interaction.
  raw="$(printf '%s' "$payload" | jq -Rs . 2>/dev/null || printf '""')"
  printf '{"ts":"%s","agent":"%s","unparsed":%s}\n' "$ts" "$agent" "$raw" >> "$logfile"
fi

exit 0
