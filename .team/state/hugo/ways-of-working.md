# hugo — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Pass issue-opener email down at assignment time

Owner standing rule (tg 2114, 2026-05-12, delivered via otto):

> "Whenever any of the engineers are working on a issue they should
> ask themselves who opened this issue. and the 'correct' email of
> that person must be included in the commits of that PR as co
> author (find the right way to do that). in first place Hugo
> should pass this information down to them when assigning tasks"

**Why:** every issue-driven PR should credit the issue author on
every commit, not just thank them in the PR body. The bottleneck
is finding the right email — engineers shouldn't each re-do that
lookup. Owner wants me to resolve it once at assignment time and
hand it down.

**How to apply:**

- Every ticket I assign (DM to engineer): include three fields —
  issue number, opener's GH login, opener's **correct** email for
  the Co-Authored-By trailer.
- Email lookup, in order:
  1. `gh api /users/<login> --jq .email` — public email if set.
  2. If null: `gh api '/repos/Alireza29675/teamctl/commits?author=<login>&per_page=1' --jq '.[0].commit.author.email'` — most recent commit email (works only if they've contributed before).
  3. If still null: GitHub no-reply form — `gh api /users/<login> --jq '"\(.id)+\(.login)@users.noreply.github.com"'`. Always works, always GitHub-recognised.
  4. If `<login>` is an internal teammate (ada/hugo/kian/neda/nico/otis/sage/wren), there is no human behind it — skip the trailer entirely.
- Name: `gh api /users/<login> --jq .name` (fall back to login if null).
- Format I hand down: `Co-Authored-By: <Name> <<email>>` — exactly that, ready to paste into commits.
- Carve-out: if I forget to include it and the engineer self-picks
  from the board, the engineer does the lookup themselves per
  their ways-of-working entry. I am the primary source, not the
  single source.

