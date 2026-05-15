# wren — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Credit the issue opener as Co-Authored-By on every commit

Owner standing rule (tg 2114, 2026-05-12, delivered via otto):

> "Whenever any of the engineers are working on a issue they should
> ask themselves who opened this issue. and the 'correct' email of
> that person must be included in the commits of that PR as co
> author (find the right way to do that). in first place Hugo
> should pass this information down to them when assigning tasks"

**Why:** issue authors deserve commit-level credit, not just a
PR-body thanks. Credit travels through git history; PR bodies
don't. Applies to every issue-driven PR, internal-filer or
external-contributor alike (with one carve-out: see below).

**How to apply:**

- **First place is hugo.** Hugo's assignment DM should already
  contain a ready-to-paste `Co-Authored-By: <Name> <<email>>`
  line. If it does, use that verbatim on every commit of the PR.
- **Fallback (hugo forgot, or I self-picked from the board):** do
  the lookup myself, in order:
  1. `gh api /users/<login> --jq .email` — public email.
  2. If null: `gh api '/repos/Alireza29675/teamctl/commits?author=<login>&per_page=1' --jq '.[0].commit.author.email'` — prior-commit email.
  3. If still null: GitHub no-reply form via `gh api /users/<login> --jq '"\(.id)+\(.login)@users.noreply.github.com"'`. Always works.
  - Name via `gh api /users/<login> --jq .name` (login as fallback).
- **Skip if opener is an internal teammate** (ada/hugo/kian/neda/
  nico/otis/sage/wren) — no human behind the agent login.
- **Every commit on the PR**, not just the merge commit. Owner
  said "the commits of that PR" (plural).
- **Format** (literal, blank line before trailer block):

      <commit subject>

      Co-Authored-By: <Name> <<email>>

## Direct-to-owner routing (owner standing rule, Telegram msg 1422, 2026-05-11)

> "PR links go from authoring engineer DIRECTLY to owner via reply_to_user — not through hugo. Questions for owner go DIRECTLY from you to owner — not through hugo as a relay. Hugo gives owner high-level overviews only, not per-PR detail."

- **Why:** hugo's PM bandwidth is finite; per-PR relay duplicates effort and adds latency. Owner wants a chat thread per engineer-PR for live review/merge, which only works if the engineer opens the thread directly.
- **How to apply:**
  - The moment a PR opens, ping owner via `reply_to_user` with the URL + one-line read. Don't wait for hugo's PM overview.
  - Any variant/scope/design question for owner goes directly via `reply_to_user`, not via `dm hugo`.
  - Hugo still gets a coordination DM ("PR up, ready for qa", "shipped + archived", "blocker on X") — that's the high-level lane and stays.
  - Cascade-night precedent (2026-05-11): kian, ada, wren all DM'd owner directly on their PRs.
- **Escalation carve-out (owner, Telegram msg 1423, 2026-05-11):** *"unless you really want to escalate."* — urgent / blocked / strong-disagreement cases may still route through hugo. Default = direct; escalation = via hugo when that adds weight (e.g. requesting capacity reshuffle, raising a release blocker, dissenting from a ratified design).

## Reproducing macOS bash 3.2 quirks on Linux (team-pattern, hugo h/t otis, 2026-05-11)

> "macOS bash 3.2 ≅ `bash --posix -O compat32` on Linux for repro. Any shell-script bug that only fires on macOS `/bin/sh` (bash 3.2) and won't reproduce on Linux: run `bash --posix -O compat32 -n <script>` on the Linux box. Triggers the same parser quirks byte-identically without needing a macOS machine. Found during T-190 hotfix — reproduced owner's exact `unexpected EOF while looking for matching '}'` from `${VAR:=DEFAULT}` parser fragility this way."

- **Why:** macOS ships bash 3.2 as `/bin/sh` (Apple's GPL3 holdout); engineers without a Mac at hand can't reproduce a shipped regression directly. compat32 + posix flags emulate the parser-version quirks closely enough for byte-identical repro.
- **How to apply:**
  - First move for any "fires only on macOS shell" report: `bash --posix -O compat32 -n <script>` for syntax/parse errors, drop `-n` to actually execute.
  - Combine with `set -u` to surface unbound-variable cases the macOS shell catches earlier than recent bash.
  - Pin the workaround at the test layer when a fix lands — a Linux test that asserts the script parses under compat32 closes the loop without needing a macOS runner.

## "Bug doesn't repro" is data, not noise (team-pattern, hugo h/t otis, 2026-05-11)

> "When a ticket says X is broken and the TDD-first probe shows it isn't broken in the described shape: surface honestly with tests in worktree ready, don't manufacture a fix to satisfy the ticket. Right move is to ask owner for a different repro or close as not-a-bug. Bench while the question resolves. (T-182 lineage.)"

- **Why:** Filing-the-fix engineers sometimes catch a ticket whose reported behavior doesn't reproduce in current main. Inventing a fix to match the words wastes a PR slot and risks introducing a real regression. The honest "I can't repro" report is the signal owner needs to either tighten the repro or close the ticket — both are progress.
- **How to apply:**
  - TDD probe first: write the test that would fail if the bug were real. If it passes on main, you have data.
  - DM owner directly (per the routing standing rule) with the repro you tried + the unexpected-green test output. Offer worktree access if useful.
  - Stay benched on that ticket while owner thinks. Don't pre-stage speculative fixes — they bias the conversation.
  - If owner closes as not-a-bug, archive the probe-test if it pins meaningful behavior; discard if it pins nothing.

