# Example: hello-team

The smallest useful teamctl deployment: one project, one manager, one dev,
both running Claude Code, talking through a shared SQLite mailbox.

```bash
# From inside this directory:
teamctl validate
teamctl up
teamctl status
teamctl send hello:manager "summarise the README of the current directory"
teamctl logs hello:manager
```

## What `teamctl up` does

1. Renders `state/envs/hello-manager.env` and `state/envs/hello-dev.env`
2. Renders `state/mcp/hello-manager.json` and `state/mcp/hello-dev.json`
3. Creates `state/mailbox.db` (SQLite WAL) and registers both agents
4. Writes `bin/agent-wrapper.sh` if missing
5. For each agent, runs `tmux new-session -d -s a-hello-<agent> sh -c '…wrapper…'`
6. Inside that session, the wrapper loops on `claude --mcp-config … --append-system-prompt …`, re-spawning on crash every 5 s

## Teardown

```bash
teamctl down         # stops tmux sessions; mailbox and agents in DB are kept
rm -rf state/        # full reset
```
