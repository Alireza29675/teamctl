# ada — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Direct-to-owner routing

Owner standing rule (tg 1422, broadcast by hugo 2026-05-11):

> PR links go from authoring engineer DIRECTLY to owner via
> reply_to_user — not through hugo.
> Questions for owner go DIRECTLY from engineer to owner — not
> through hugo as a relay.
> Hugo gives owner high-level overviews only, not per-PR detail.

**Why:** Hugo's relay layer was adding latency on cascade-night
and stripping context. Owner wants one-hop comms with the
engineer who wrote the code.

**How to apply:** Every new PR I open gets a `reply_to_user`
one-liner with the URL the moment it opens. Variant / scope
/ design questions go straight to owner via `reply_to_user`,
not via `dm hugo`. Hugo still owns routing, qa, capacity,
release coordination — but is not in the question path.

**Escalation carve-out** (owner tg 1423, hugo broadcast id 1427):
*"unless you really want to escalate."* Urgent / blocked /
strong-disagreement cases can route through hugo. Default is
direct; escalation is the opt-in path.

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

## "Bug doesn't repro" is data, not noise

Hugo team-pattern broadcast id 1626 (h/t otis, T-182 lineage):

> When a ticket says X is broken and the TDD-first probe shows
> it isn't broken in the described shape: surface honestly with
> tests in worktree ready, don't manufacture a fix to satisfy
> the ticket. Right move is to ask owner for a different repro
> or close as not-a-bug. Bench while the question resolves.

**Why:** Manufacturing a fix to a non-existent bug ships dead
code and pollutes the diff; worse, it can mask the real bug if
there is one in a different shape. Honesty about no-repro is
both engineering hygiene and a request for better data.

**How to apply:** TDD-first on any "X is broken" ticket. If the
failing test doesn't fail as described, document the trace,
keep tests in worktree, DM owner with "can't repro from issue
example — here's the trace; do you have a different shape in
mind?" Bench until reply.
