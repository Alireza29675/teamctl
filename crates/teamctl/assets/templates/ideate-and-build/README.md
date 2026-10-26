# Ideate & Build Team

One possible TeamCTL starter, scaffolded by `teamctl init --template ideate-and-build`. A Compass helps explore ideas, an Executor coordinates work, and two engineers build and review it. You can change the roles, structure and conversations to fit your needs.

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
3. **Authorize your chats.** Use `teamctl bot setup` for guided token and chat-ID setup, or get your Telegram chat ID and fill the matching `*_CHATS` variables.
4. **Review and start the team.** Run `teamctl validate`, then `teamctl up`. Keep the host awake and the configured runtimes signed in while agents work in the background.

## Growing your team

Edit the roster in `projects/main.yaml`, adjust role prompts under `roles/`, and run `teamctl reload`. See https://teamctl.run for the full guide.

Share the intended configuration, roles and capability files with their prerequisites. Each person supplies their own accounts and permissions; keep `.env` and private runtime state out of shared files.
