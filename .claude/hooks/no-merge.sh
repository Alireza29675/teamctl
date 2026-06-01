#!/usr/bin/env bash
# Hard guard — this team never merges or pushes to the trunk on its own.
# Shipping is the operator's call, full stop.
#
# This is a PreToolUse hook: it denies the tool call at the tool layer
# and fires even under --dangerously-skip-permissions, so it holds no
# matter what a role prompt says (or forgets) and no matter how the
# command is phrased. It applies only to teamctl agents (AGENT_ID is
# set by the wrapper) — the operator's own `claude` sessions in this
# repo can merge freely.
#
# Blocks: `gh pr merge`, `git merge` (the workflow rebases, it never
# merges), and any `git push` aimed at main/master.

set -u

[ -n "${AGENT_ID:-}" ] || exit 0          # not a teamctl agent → allow

payload="$(cat)"
[ -n "$payload" ] || exit 0

tool="$(printf '%s' "$payload" | jq -r '.tool_name // ""' 2>/dev/null)"
[ "$tool" = "Bash" ] || exit 0            # merges happen via the shell

cmd="$(printf '%s' "$payload" | jq -r '.tool_input.command // ""' 2>/dev/null)"
[ -n "$cmd" ] || exit 0

blocked="$(printf '%s' "$cmd" | perl -ne '
  # Test each shell-separated segment on its own so a compound like
  # `git rebase main && git push --force origin T-9/x` does not trip the
  # push guard on the unrelated `main` token in the rebase segment.
  for my $seg (split m{(?:&&|\|\||;|&|\|)}) {
    if ($seg =~ /\bgh\s+pr\s+merge(?:\s|$)/i
        or $seg =~ /\bgit\s+merge(?![-\w])/i
        or ($seg =~ /\bgit\s+push\b/i
            # block only when main/master is the push DESTINATION ref
            # (a space- or colon-delimited whole word), not any substring:
            # blocks `push origin main`, `push origin HEAD:main`, `push origin :master`;
            # allows `push origin T-9/x`, `push origin feat/main-menu-fix`.
            and $seg =~ m{(?:\s|:)(?:refs/heads/)?(?:main|master)(?:\s|$)}i)) {
      print "x"; last;
    }
  }
')"

if [ -n "$blocked" ]; then
  reason="Blocked by team policy: this team never merges or pushes to the trunk on its own — shipping is the operator's call. Own the PR and hand it to the operator (via hugo) to merge. (Hard guard: .claude/hooks/no-merge.sh)"
  printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":%s}}\n' \
    "$(printf '%s' "$reason" | jq -Rs .)"
fi
exit 0
