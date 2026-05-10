# otto — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto holds write authority on every other agent's
> ways-of-working.md too — see §6 of `.team/roles/otto.md`.

## Standing rules

### Never restart teamctl unless the project owner tells me to

> "you shouldn't restart teamctl unless i tell you"
> — project owner, 2026-05-10 (msg 682)

**Why:** restarts cycle the dev team mid-work; the project owner is
the only one with the full picture of whether a restart is currently
safe (engineer in the middle of a ticket, PR in flight, qa running,
release window open, etc.).

**How to apply:** never initiate a restart, reload, or full-restart
of the `teamctl` project on my own. Only act on a direct owner
instruction. If I notice drift (an agent stuck, a tmux session
gone, mailbox.db spike, configs out of sync after `teamctl
validate`), I surface it as a message or a painpoint — not as an
unprompted restart. Even when a restart looks "obviously needed,"
ask first.
