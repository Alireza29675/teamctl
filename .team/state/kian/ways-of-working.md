# kian — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## PR links and questions go direct to owner

Owner standing rule, relayed by hugo on `#dev` 2026-05-11 (tg
1422 + carve-out 1423):

> "PR links go from authoring engineer DIRECTLY to owner via
> reply_to_user — not through me. Questions for owner go DIRECTLY
> from you to owner — not through me as a relay. I give owner
> high-level overviews only, not per-PR detail. **Unless you
> really want to escalate** — escalation cases (urgent / blocked
> / strong disagreement) can route through me."

**Why:** owner wants per-PR / per-question signal coming from the
authoring engineer with full context, not flattened through hugo.
Hugo stays the coordinator (routing, qa, capacity, release), not
the question-relay.

**How to apply:**

- Every PR I open → `reply_to_user` direct to owner with the URL
  the moment it lands, plus a one-line read of the change.
  Independently, DM hugo so qa can run.
- Variant / scope / design questions → DM owner directly.
- Idle / ready ping after compact → `reply_to_user` direct.
- Escalation only (release blocker / strong disagreement / I
  need air-cover) → can route through hugo.

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

## macOS bash 3.2 reproducible on Linux via `bash --posix -O compat32`

Team pattern (h/t otis, validated on T-190), broadcast on `#dev`
2026-05-11 by hugo (msg 1626):

> "Any shell-script bug that only fires on macOS `/bin/sh` (bash
> 3.2) and won't reproduce on Linux: run
> `bash --posix -O compat32 -n <script>` on the Linux box.
> Triggers the same parser quirks byte-identically without needing
> a macOS machine."

**Why:** macOS ships bash 3.2 as `/bin/sh` (license reasons).
Bash 3.2 has parser bugs (e.g. `${VAR:=DEFAULT}` with escape
sequences in DEFAULT) that bash 4+ and dash don't have. Linux CI
runners use bash 4+ or dash, so wrapper bugs that only bite macOS
ship silently. We don't currently have a macOS CI runner.

**How to apply:**

- ANY change to `crates/teamctl/assets/agent-wrapper.sh` (or any
  shell script that ships) → run
  `bash --posix -O compat32 -n <script>` locally before push.
- When triaging a "agents come up then immediately stop" report:
  use this command to repro on a Linux box. If it errors, the
  fix is bash-3.2 portability, not anything else.
- Sage holds the canonical team-pattern doc; this entry is my
  per-engineer copy.

## "Bug doesn't repro" is data, not noise

Team pattern (T-182 lineage), broadcast on `#dev` 2026-05-11 by
hugo (msg 1626):

> "When a ticket says X is broken and the TDD-first probe shows
> it isn't broken in the described shape: surface honestly with
> tests in worktree ready, don't manufacture a fix to satisfy the
> ticket. Right move is to ask owner for a different repro or
> close as not-a-bug. Bench while the question resolves."

**Why:** shipping a fix for a non-existent bug adds untested
behavior change, hides the real (different) issue if there is
one, and burns review cycles. A "can't repro" with TDD probe in
a worktree is genuinely useful evidence — owner can ratify
"close as not-a-bug" or provide a fresh repro, both fast.

**How to apply:**

- If TDD probe doesn't reproduce the described failure: stop.
  Don't reach for a "looks-plausible" fix. Surface the
  not-reproducing finding to hugo + owner with the probe still
  in worktree.
- Offer next steps: (a) is there a different repro path? (b)
  close as not-a-bug? (c) ship the probe-as-defensive-test even
  though no fix is needed?
- Bench while owner ratifies. Don't manufacture work to look busy.
