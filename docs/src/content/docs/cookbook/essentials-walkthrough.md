---
title: Essentials walkthrough
description: Start an ops agent that helps define and adjust your team.
---

The `essentials` template is one possible TeamCTL starting point. It creates a blank `main` project and an `ops` agent whose role is to help populate it. Describe the team on Telegram, review the proposed roles and inspect the files it writes.

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
│   └── ops.yaml               # the ops agent lives here
├── roles/
│   └── ops.md                 # ops agent's voice + boundaries
├── .env.example
├── .gitignore
└── README.md
```

## 2. Connect the ops agent to Telegram

The ops agent talks to you through Telegram. Three steps:

1. Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.
2. Get your Telegram chat id ([@userinfobot](https://t.me/userinfobot) is the quickest).
3. Copy `.env.example` to `.env` and fill in:

```bash
cp .team/.env.example .team/.env
# edit .team/.env
TEAMCTL_TG_OPS_TOKEN=123456:AAH-...
TEAMCTL_TG_OPS_CHATS=12345678
```

## 3. Bring the team up

```bash
teamctl validate
teamctl up
```

`teamctl status` should show the ops agent running. The `main` project starts without agents. Keep the host awake while the helper works in the background.

## 4. Define your team with the ops agent

Open Telegram, find your bot, and message it. Loose framing is fine:

> *I want a team that helps me triage GitHub issues for my OSS project. Two roles I can think of: someone who reads the issue and labels it, someone who drafts a reply.*

The ops role instructs it to:

- Draft a roster (one manager + two workers, channel and ACL shape, role-prompt sketches).
- Run the draft past you on Telegram and ask for tweaks.
- Edit `projects/main.yaml` and create `roles/triage.md` + `roles/responder.md`.
- Run `teamctl reload` so the new agents come online without restarting the ops agent itself.
- Confirm the new agents are running and offer to send a first message to one of them on your behalf.

## 5. Evolve

Come back to the ops agent when you want to:

- Add another agent (*"add a critic that reviews the responder's drafts before they go out"*).
- Retire one (*"retire the labeler — I'm doing that part by hand now"*).
- Swap a model (*"move the responder to claude-sonnet-4-6, it's cheaper for that volume"*).
- Investigate a stuck agent (*"the triage agent hasn't moved on its latest task in 20 minutes — show me its log"*).

The ops agent edits the same `projects/main.yaml` and `roles/` files you can edit by hand. Inspect its changes with `git diff`, then review and adjust them as needed.

## Start without the helper

If you prefer to write the definition, `teamctl init my-team --template blank --yes` creates a global configuration and an empty `projects/main.yaml`. Define peers, specialists or reporting relationships around your work.

You can share the intended configuration and role files once they are useful. Include prerequisites and local paths; leave credentials and private runtime state out of the shared setup.
