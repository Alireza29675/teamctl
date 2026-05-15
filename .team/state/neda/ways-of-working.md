# neda — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Verify contributor identifiers before any public-facing artifact (2026-05-11)

> "be very careful with people's names and emails and social links. don't use what you're unsure. always double check!" — owner, tg 1496

**Why:** got the public-facing release body for v0.8.0 wrong by writing `@HamedFathi` (display-name guess) instead of `@hamifthi` (actual github handle). Cascade was held until corrected. Public-facing artifacts mis-crediting outside contributors are an unforced trust-erosion.

**How to apply:** before any public-facing artifact (release body, README, docs page, social post, blog) mentions a contributor:
- Pull the handle from the **commit co-author trailer** (`git log --format='%(trailers:key=Co-authored-by)' <range>`) OR
- From `gh pr view <N> --json author --jq '.author.login'` OR
- From an **owner-supplied profile URL** (extract the username path segment, e.g. `https://github.com/hamifthi` → `hamifthi`)
- Never guess a handle from a display name (Display "Hamed Fathi" ≠ handle `HamedFathi`)
- Same rule for emails (use the trailer email verbatim, scrub `<id+handle@users.noreply.github.com>` to bare `@handle` for public link)
- When unsure, ask the owner or PM. Don't ship.

## Worktrees for all development (2026-05-11)

> "you should also work with worktrees like engineers btw. the main teamctl should remain main branch and development should happen in worktrees" — owner, tg 1287

**Why:** the main checkout at `/home/alireza/dev/projects/teamctl/` should stay on `main` so any quick read / status check shows clean trunk state. Development on feature branches happens in `.worktrees/<short-name>/` so multiple in-flight branches don't clobber each other.

**How to apply:**
- Before starting any non-trivial change, create a worktree off `origin/main`: `git worktree add .worktrees/<short-name> -b <branch-name> origin/main`.
- Work, commit, push, file PR — all from the worktree path.
- Never let the main checkout drift off `main` for any significant work.
- After PR merges, prune the worktree: `git worktree remove .worktrees/<name>` (ask owner first if uncertain).
- The owner's `feedback_no_em_dash.md` rule still applies inside worktree files too.

## Sage relays = awareness only (2026-05-11)

> "a lot of times I am just talking and brainstorming with Sage about my future visions, and Sage might also send some of them to you so you are just aware of the future, which is upcoming. But it shouldn't be affecting any of our docs or anything like that, because many of them are hypothetical. If you are in doubt, just ask Sage or me." — owner, tg 1266

**Why:** sage is co-thinker on vision; most of what she forwards is hypothetical, not ratified positioning. Leaking it into README/docs/website would over-commit the project.

**How to apply:** default = read, note, do not write. Never edit user-facing copy based on a sage relay alone. Material moves from awareness to actionable only via owner explicit ratify. When in doubt, ask sage or owner.
