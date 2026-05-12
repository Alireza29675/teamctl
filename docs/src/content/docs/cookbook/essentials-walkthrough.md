---
title: Essentials walkthrough — let the builder build your team
description: A full first-run with the Essentials template — scaffold, wire Telegram, chat with the builder, watch it scaffold your team.
---

The `essentials` template ships a builder bot whose job is to evolve your team for you on Telegram. Instead of hand-writing `projects/main.yaml` and a stack of role files, you describe the team you want in plain English and the builder lays it down.

This recipe walks an end-to-end first-run.

## 1. Scaffold

```bash
teamctl init my-team --template essentials --yes
cd my-team
```

You get:

```
.team/
├── team-compose.yaml          # broker + supervisor + both projects
├── projects/
│   ├── main.yaml              # your project — starts blank
│   └── ops.yaml               # the builder lives here
├── roles/
│   └── builder.md             # builder's voice + boundaries
├── .env.example
├── .gitignore
└── README.md
```

## 2. Wire the builder's Telegram bot

The builder talks to you through Telegram. Three steps:

1. Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.
2. Get your Telegram chat id ([@userinfobot](https://t.me/userinfobot) is the quickest).
3. Copy `.env.example` to `.env` and fill in:

```bash
cp .team/.env.example .team/.env
# edit .team/.env
TEAMCTL_TG_BUILDER_TOKEN=123456:AAH-...
TEAMCTL_TG_BUILDER_CHATS=12345678
```

## 3. Bring the team up

```bash
teamctl validate
teamctl up
```

`teamctl ps` should show one tmux session for the builder (`a-ops-builder`). Your `main` project has zero agents — that's by design.

## 4. Chat with the builder

Open Telegram, find your bot, and message it. Loose framing is fine:

> *I want a team that helps me triage GitHub issues for my OSS project. Two roles I can think of: someone who reads the issue and labels it, someone who drafts a reply.*

The builder will:

- Draft a roster (one manager + two workers, channel and ACL shape, role-prompt sketches).
- Run the draft past you on Telegram and ask for tweaks.
- Edit `projects/main.yaml` and create `roles/triage.md` + `roles/responder.md`.
- Run `teamctl reload` so the new agents come online without restarting the builder itself.
- Confirm the new agents are running and offer to send a first message to one of them on your behalf.

## 5. Evolve

Come back to the builder any time you want to:

- Add another agent (*"add a critic that reviews the responder's drafts before they go out"*).
- Retire one (*"retire the labeler — I'm doing that part by hand now"*).
- Swap a model (*"move the responder to claude-sonnet-4-6, it's cheaper for that volume"*).
- Investigate a stuck agent (*"the triage agent hasn't moved on its latest task in 20 minutes — show me its log"*).

The builder edits the same `projects/main.yaml` and `roles/` files you'd edit by hand, so you can always inspect what it did with `git diff` and adjust by hand if you disagree.

## When to skip the builder

If you already know exactly what team you want and you'd rather wire it by hand, `teamctl init my-team --template blank --yes` gives you the bare `team-compose.yaml` + empty `projects/main.yaml`. The `essentials` builder is opt-in, not assumed.
