# Interfaces

An **interface** is a pluggable human-facing channel. Telegram is one adapter; Discord, iMessage, CLI, and webhook are others. You can configure many at once — a manager might receive DMs on Telegram and approvals on iMessage.

```yaml
interfaces:
  - type: telegram
    name: tg-main
    config:
      bot_token_env: TEAMCTL_TELEGRAM_TOKEN
      authorized_chat_ids: [75473051]

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

Each manager declares which interfaces it receives from. Approvals are routed to every attached interface until one of them decides.

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

1. Reads new messages for subscribed managers (`inbox_peek` / `inbox_watch`, filtered by `telegram_inbox: true` etc).
2. Surfaces them to the human.
3. Writes human replies back as `sender=user:<handle>` via SQL insert or team-mcp.
4. Listens for approval notifications and renders inline decision UI.

Adapters ship as small standalone binaries next to `teamctl` — `team-interface-telegram`, `team-interface-discord`, etc.

## Related

- [HITL](./hitl.md)
- [Guide: Telegram bot](../guides/telegram-bot.md)
