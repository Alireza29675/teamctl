# nico — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Direct-to-owner PR links + questions (owner standing rule, relayed via hugo tg 1422 / channel msg 1426, 2026-05-11)

> "PR links go from authoring engineer DIRECTLY to owner via reply_to_user — not through me. Questions for owner go DIRECTLY from you to owner — not through me as a relay. I give owner high-level overviews only, not per-PR detail."

**Why:** owner wants the actionable thread with the engineer who wrote the diff — saves a PM hop on cascade-night merges and keeps the conversation with the person who can answer.

**How to apply:** every PR I open → `reply_to_user` to owner with the URL the moment it opens (matches the existing "PR-link surface" pattern). Variant/scope/design questions on my ticket → `reply_to_user` straight to owner, not `dm hugo`. Hugo still gets a `dm` for qa routing + the high-level overview slot, just not as a relay for owner-bound traffic. Precedent set on T-169 (#175) — kept doing it; the rule formalizes that this is the shape going forward.

**Carve-out (owner tg 1423, relayed hugo msg 1427):** *"unless you really want to escalate."* Urgent / blocked / strong-disagreement cases may route through hugo. Default stays direct; hugo is the escalation lane, not the default lane.

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

## macOS bash 3.2 reproducer on Linux (hugo msg 1626, h/t otis, 2026-05-11)

> `bash --posix -O compat32 -n <script>` on Linux triggers the same parser quirks as macOS `/bin/sh` (bash 3.2), byte-identically.

**Why:** macOS-only shell-script bugs are painful to repro without a Mac. Found during the T-190 v0.8.1 hotfix — owner's `${VAR:=DEFAULT}` parser fragility (`unexpected EOF while looking for matching '}'`) reproduced exactly with the compat flags on Linux.

**How to apply:** any ticket where a bash/sh script "only breaks on macOS" and Linux repro feels blocked, reach for `bash --posix -O compat32 -n <script>` first. `-n` parses without executing (catches parser-level errors); drop it to actually run. Saves filing for a Mac box or waiting on owner-side repro.

## "Bug doesn't repro" is data, not noise (hugo msg 1626, T-182 lineage, 2026-05-11)

> When a TDD-first probe shows the ticket's described bug isn't actually broken in that shape: surface honestly with the probe tests ready in a worktree; don't manufacture a fix to satisfy the ticket.

**Why:** ticket-titles ≠ ground-truth. Fixing what isn't broken (a) ships untested behavior changes, (b) wastes the cascade window, (c) leaves the real underlying issue undiagnosed. T-182 played out exactly this shape; owner closed as not-a-bug after the engineer surfaced the non-repro instead of force-fitting a change.

**How to apply:** when my first move on a ticket is a TDD probe and the probe shows the described shape is already correct: stop coding, surface to owner via `reply_to_user` with the probe tests + my read, and ask for a different repro or close-as-not-a-bug. Stay in worktree, ready to act. Bench is honest, not lazy.
