# {{project_name}}

This `.team/` was scaffolded by `teamctl init --template essentials`. You get two projects out of the box:

- **`main`** — your project. It starts blank, with zero agents. This is where the team you're building lives.
- **`ops`** — the helper project. It ships with a single `ops` agent whose job is to populate `main` for you. Think of it as the team that helps build your team.

## First steps

1. **Wire the ops agent's Telegram bot.**
   - Create a bot via [@BotFather](https://t.me/BotFather) and grab the token.
   - Get your Telegram chat id ([@userinfobot](https://t.me/userinfobot) is the quickest).
   - Copy `.env.example` to `.env` and fill in `TEAMCTL_TG_OPS_TOKEN` + `TEAMCTL_TG_OPS_CHATS`.
2. **Bring the team up.**
   ```bash
   teamctl validate
   teamctl up
   ```
3. **Message the ops agent on Telegram.** Say what you want — *"I want a team that helps me triage GitHub issues for my OSS project"* or *"I want a research buddy that summarizes papers."* The ops agent will draft a roster, run it past you, then scaffold it under `projects/main.yaml`.

## Day-to-day

- Talk to the ops agent when you want to add, retire, or reshape an agent. It edits `projects/main.yaml`, role files under `roles/`, and documents env vars in `.env.example` directly — then reloads so the change is live.
- Edit `projects/main.yaml` and `roles/*.md` by hand any time you prefer — both you and the ops agent share write access.
- Top-level changes (broker, supervisor, interface blocks in `team-compose.yaml`) stay yours alone. The ops agent reads `team-compose.yaml` to answer questions but won't edit it; for those changes the docs are the canonical surface.
- Avoid editing `projects/ops.yaml` or `roles/ops.md` — that's the ops agent's own scope, and reshaping it can leave you without a helper.

## Stop

```bash
teamctl down            # stop tmux sessions; mailbox preserved
rm -rf state/           # full reset
```

## Customize

- Edit `roles/ops.md` to change how the ops agent talks to you (voice, authority, what it offers).
- Edit `projects/ops.yaml` to swap the ops agent's model, add another helper, or tune its Telegram setup.
- Edit `team-compose.yaml` to wire additional interfaces or change broker / supervisor.

After any edit, `teamctl reload` picks up the change.
