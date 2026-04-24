# Your first team

The `examples/hello-team/` directory is the smallest useful deployment. Two agents, one project, Claude Code on both, nothing fancy.

```
hello-team/
├── team-compose.yaml          # broker, supervisor, project list
├── projects/
│   └── hello.yaml             # one manager, one dev, one channel
└── roles/
    ├── manager.md             # manager system prompt
    └── dev.md                 # dev system prompt
```

## Run it

```bash
cd examples/hello-team
teamctl validate           # ok · 1 project · 2 agents
teamctl up                 # renders state/, starts tmux sessions
teamctl status             # shows both agents running
teamctl send hello:manager "hi"
teamctl logs hello:manager
```

## What got created under `state/`

- `state/envs/hello-<agent>.env` — environment for the agent wrapper
- `state/mcp/hello-<agent>.json` — MCP config pointing at `team-mcp`
- `state/mailbox.db` — SQLite mailbox

## Change something

Edit `roles/manager.md` and run `teamctl reload` — only the manager restarts. The dev is untouched. Edit the compose tree to add a second dev; `reload` picks it up.

## What's next

- Add a [Telegram bot](./telegram-bot.md) so you can DM the manager from your phone.
- Read about [channels](../concepts/channels.md) to wire up broadcast groups.
- See the bigger `multi-agent`, `multi-runtime`, `two-projects` examples for more realistic shapes.
