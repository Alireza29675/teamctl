# Ideate & Build Team

A four-agent team scaffolded by `teamctl init` (ideate-and-build template): think an idea through with a dedicated partner, then build it with a small engineering crew.

## What's here

- **`team-compose.yaml`** — the team definition: broker, defaults, and the list of projects.
- **`projects/main.yaml`** — the roster: an Executor, a Compass, and two engineers.
- **`roles/`** — the cascading role prompts (`_base.md` shared by all, `_telegram.md` for the managers, `_engineer.md` for the builders, plus one file per manager).
- **`charter.md`** — the shared source of truth every agent re-reads each loop. The Executor keeps it current.
- **`.env.example`** — copy to `.env` and fill in your Telegram bot tokens.

## The team

- **Executor** — runs the team on your behalf. Talks to you on Telegram, turns what you want into work, delegates to the engineers, reports back.
- **Compass** — your upstream ideation partner on a *separate* private bot. Helps you figure out what to build next and shapes it before it ever reaches the build team. Hands a shaped idea to the Executor only when you say so.
- **Two engineers** — the builders. They pick up work from the Executor, build it, and ship it.

## Getting started

1. **Create two Telegram bots.** Message [@BotFather](https://t.me/BotFather), run `/newbot` twice — one bot for the Executor, one for Compass — and copy each token.
2. **Configure secrets.** Copy `.env.example` to `.env` and paste the tokens into `TEAMCTL_TG_EXECUTOR_TOKEN` and `TEAMCTL_TG_COMPASS_TOKEN`.
3. **Find your chat IDs.** Start a chat with each bot, then run `teamctl status` — it'll show the chat IDs. Put them in the matching `*_CHATS` variables.
4. **Bring the team up.** Run `teamctl up`. Your Executor and Compass are now live on Telegram.

## Growing your team

Edit the roster in `projects/main.yaml`, adjust role prompts under `roles/`, and run `teamctl reload`. See https://teamctl.run for the full guide.
