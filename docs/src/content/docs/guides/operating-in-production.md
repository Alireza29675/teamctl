---
title: Operating in production
---

## Supervisor back-ends

The default `TmuxSupervisor` works on any host with `tmux` installed. The agent wrapper loops on the runtime binary, so crashes restart within 5 s. What you **don't** get is reboot-survivability.

For 24/7 home-lab use, switch to a system supervisor:

```yaml
# team-compose.yaml
supervisor:
  type: systemd        # Linux (user-scope units)
  # or: type: launchd  # macOS (per-user LaunchAgents)
```

The `Supervisor` trait abstracts the back-end; agent lifecycle, `teamctl status`, and `teamctl reload` work the same regardless.

## State on disk

Everything runtime-ish lives under `state/`:

```
state/
├── mailbox.db                   # SQLite, WAL mode
├── envs/<project>-<agent>.env   # rendered per-agent env
├── mcp/<project>-<agent>.json   # MCP stdio config
└── applied.json                 # last-reloaded compose hash
```

Back it up the way you back up any SQLite DB (VACUUM INTO, litestream, or copy while the agents are quiesced with `teamctl down`).

## Garbage collection

`teamctl gc` drops acked messages older than `budget.message_ttl_hours` (default 24) and marks expired pending approvals. Run it on a cron:

```
*/30 * * * * /usr/local/bin/teamctl -C /srv/teamctl gc
```

## Observability

- `teamctl status` — agent state + inbox depth per agent.
- `teamctl budget` — 24 h message / approval / USD counts per project.
- `teamctl bridge list` — open and recently expired inter-project links.
- `tmux attach -t a-<project>-<agent>` — raw runtime TTY (read-only recommended).
- `TEAMCTL_LOG=debug teamctl up` — verbose tracing from the control plane.

## Security posture

- Give each interface adapter the minimum token scope it needs — Telegram bots can be restricted by `authorized_chat_ids`.
- The HITL gate denies sensitive actions by default. Don't remove items from `hitl.globally_sensitive_actions` — add `auto_approve_windows` with tight `until:` when you need time-limited automation.
- Runtimes (`claude`, `codex`, `gemini`) can do whatever you permit them to do. Use `permission_mode: plan` to make an agent read-only.

## When something goes wrong

- Agent keeps restarting → `teamctl logs <project>:<agent>` shows the wrapper and runtime output.
- Mailbox grows unbounded → `teamctl gc`; check `message_ttl_hours`.
- Agents idle with full inbox → `teamctl status` inbox depth > 0 but no activity usually means the prompt doesn't instruct them to call `inbox_watch`. Review role markdown.
