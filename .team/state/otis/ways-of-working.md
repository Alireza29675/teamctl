# otis — ways of working

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

## Owner-direct routing for PR links and questions

**Standing rule (hugo broadcast 2026-05-11, tg 1422 + 1423 carve-out):**

> "your developers should give me the PR address and raise any questions. You should only give me the high-level overviews of what's going on with the team. ... unless you really want to escalate."

**Why:** owner gets too much noise when hugo doubles every
engineer-direct ping. Engineers know their own diffs best; the
direct line answers questions faster than a relay. Hugo's value
to the owner is signal-compression, not signal-replay.

**How to apply:**
- **When I author a PR:** DM owner directly with the PR url via
  `reply_to_user` the moment it opens. Don't expect hugo to do
  it. Same on idle-after-merge ("shipped + idle, ready for next").
- **When I have a scope/variant/design question for the owner:**
  DM owner directly. Don't ask hugo to relay.
- **When I peer-review someone else's PR:** the gh pr comment IS
  the per-PR detail surface — owner reads it directly. My DM to
  the authoring engineer carries the verdict; my note to hugo (if
  any) is overview-shaped only ("queue cleared, 4/4 approved")
  not per-PR ("approved because X, Y, Z" — that lives in the gh
  comment).
- **Escalation carve-out:** urgent / blocked / strong disagreement
  with owner can route through hugo if I really want it. Default
  is direct.
- **Cascade-night precedent:** kian + ada DM'd owner directly on
  their PRs today (2026-05-11). That's the shape going forward.
