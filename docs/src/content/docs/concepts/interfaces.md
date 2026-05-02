---
title: Interfaces
---

An **interface** is a pluggable human-facing channel. Telegram is the
shipping adapter; Discord, iMessage, CLI, and webhook are planned.

Telegram lives directly on the manager and is set up by `teamctl bot
setup` (one bot per user-facing manager — DM the bot, the message
goes to the matching manager). Other adapters use the top-level
`interfaces:` array shape.

```yaml
# projects/news.yaml — Telegram on the manager itself
managers:
  head_editor:
    runtime: claude-code
    role_prompt: roles/head_editor.md
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_HEAD_EDITOR_TOKEN
        chat_ids_env: TEAMCTL_TG_HEAD_EDITOR_CHATS
```

```yaml
# team-compose.yaml — non-Telegram adapters
interfaces:
  - type: discord
    name: discord-home-lab
    config:
      bot_token_env: TEAMCTL_DISCORD_TOKEN
      server_id: "…"

  - type: imessage
    name: imsg
    config:
      my_number: "+31…"

  - type: cli
    name: local
    # No config. Reachable via `teamctl chat <project>:<manager>`.
```

Approvals are routed to every attached interface until one of them
decides.

## What interfaces do

| Interface | Inbound DMs | Approval buttons | Notes |
|---|---|---|---|
| telegram | ✓ | ✓ | Shipping adapter. |
| discord | ✓ | ✓ | Slash commands + buttons. |
| imessage | ✓ | ✗ (reply "approve 3") | macOS host only. |
| cli | ✓ | ✓ | `teamctl chat` / `teamctl pending`. |
| webhook | ✓ | ✓ | For web dashboards or IFTTT-style. |

## Adapter contract

An adapter is a process that:

1. Reads new messages for subscribed managers (`inbox_peek` / `inbox_watch`, scoped via `--manager` for the Telegram adapter).
2. Surfaces them to the human.
3. Writes human replies back as `sender=user:<handle>` via SQL insert or team-mcp.
4. Listens for approval notifications and renders inline decision UI.

Adapters ship as small standalone binaries next to `teamctl` — `team-interface-telegram`, `team-interface-discord`, etc.

## Related

- [HITL](/concepts/hitl/)
- [Guide: Telegram bot](/guides/telegram-bot/)
