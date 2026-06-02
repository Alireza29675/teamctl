# Requirements — Habit Tracker

> The product contract for this team. The **PM owns and maintains this file** from product discovery; everyone else reads it every loop and builds toward it. If your memory and this file disagree, this file wins.
>
> This is a *starting* contract for the bundled `habit-tracker/` seed app — deliberately thin, with real open questions, so the PM has genuine discovery to do on the first goal. The PM grows it as the build runs. (Repointing the team at your own product? Replace this whole file with your product's contract.)

## Goal

A small personal **habit / streak tracker**: let one person define a few habits, mark them done each day, and see their current streak. Legible, fast, no account required — it runs as a single static web page.

## v1 scope

The minimum that makes this worth using:

- Define a habit (a name; e.g. "Read 20 min").
- Mark a habit done for today; un-mark if tapped by mistake.
- Show each habit's **current streak** (consecutive days done).
- Persist locally so data survives a page reload (no backend, no login).

## Out of scope (for now)

Deliberately *not* in v1 — don't build these by accident:

- Accounts, sync, or any backend / server.
- Reminders or notifications.
- Charts, history views, or analytics beyond the current streak.
- Multiple users or sharing.

## Decisions made

- **Static web app, no backend.** Plain HTML/CSS/JS, runs by opening `index.html`. Keeps the example about the team, not a stack. — *why: lowest friction, universally legible.*
- **Local persistence via the browser.** Data lives on the device. — *why: no login, no server, still survives reloads.*

## Open questions

The real discovery surface — the PM resolves these (with the operator only where genuinely blocking):

- **Streak definition:** does a streak break at the first missed day, or is there a grace day / freeze?
- **Counts vs. binary:** is each habit a simple done/not-done, or does some habit need a count (e.g. "8 glasses of water")?
- **Day boundary:** when does "today" roll over — local midnight, or a configurable start-of-day?
- **Empty state:** what does a brand-new user see, and how do they add their first habit?
