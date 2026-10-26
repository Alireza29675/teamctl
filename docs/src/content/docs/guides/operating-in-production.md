---
title: Operating in production
---

TeamCTL runs agent sessions on your host. Keep the host awake, monitor the sessions and provide the runtime credentials they need.

## Supervision and restarts

The shipping supervisor is `tmux`, on macOS or Linux. The agent wrapper restarts a runtime after it exits, normally after a five-second wait. Rate-limit hooks can delay that restart until the configured window clears.

Closing the dashboard leaves the sessions running. Host sleep interrupts work, and a reboot stops live processes. Start the team again with `teamctl up`; on-disk state remains available, and conversation resume depends on the runtime.

Built-in `systemd` and `launchd` backends are planned. Selecting them in compose does not provide a working supervisor today. If you configure an external system service to start TeamCTL after login or reboot, you own that service's credentials, environment, lifecycle and monitoring.

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

- `teamctl status`: agent state + inbox depth per agent.
- `teamctl budget`: 24 h message and approval counts, plus the USD ledger. Runtime cost parsers are not wired, so the USD column does not measure actual provider spend.
- `teamctl bridge list`: open and recently expired inter-project links.
- `teamctl attach <project>:<agent>`: raw runtime TTY (read-write by default — keystrokes reach the live agent; pass `--ro` to observe without typing). Internally a `tmux attach -t <tmux_prefix><project>-<agent>` where `tmux_prefix` is `a-` by default and configurable in `team-compose.yaml`.
- `TEAMCTL_LOG=debug teamctl up`: verbose tracing from the control plane.

## Security posture

- Restrict Telegram bots to the chat IDs in each manager's `chat_ids_env` variable.
- Instruct agents to request approval for sensitive actions. The approval mailbox records explicit requests; `globally_sensitive_actions` and `auto_approve_windows` do not automatically intercept runtime tool calls.
- Review the permissions and tool access of each runtime (`claude`, `codex`, `opencode`, `gemini`). Permission modes differ by runtime; see [Multi-runtime teams](/guides/multi-runtime/).

## When something goes wrong

- Agent keeps restarting → `teamctl logs <project>:<agent>` shows the wrapper and runtime output.
- Mailbox grows unbounded → `teamctl gc`; check `message_ttl_hours`.
- Agents idle with full inbox → inspect the runtime, MCP connection and role instructions. Claude Code receives channel events; the other supported runtimes receive a mailbox nudge. The role still needs to explain how to read and act on mail.
