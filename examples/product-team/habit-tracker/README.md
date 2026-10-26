# Habit Tracker (seed app)

A static habit and streak tracker used as the seed project for the [TeamCTL product-team example](../README.md).

This directory is a skeleton: a titled page, an empty habit list and a stub `app.js`. The example's roles ask agents to build toward
the product contract in [`.team/requirements.md`](../.team/requirements.md):
define a habit, mark it done each day, see your current streak — all
persisted locally, with no backend.

## Run it

It's a static page — no build step:

```bash
open index.html        # macOS
# or serve it: python3 -m http.server   → http://localhost:8000
```

## Files

- `index.html` — the page shell.
- `app.js` — the application entry point (a stub the team builds out).
- `styles.css` — base styles.

## Swap in your own product

Nothing here is load-bearing. To aim the team at your *own* repo, repoint one
line — `cwd` in `.team/projects/product-team.yaml` — at your project, and
replace `.team/requirements.md` with your product's contract. The four-agent
team is unchanged; only the target moves.
