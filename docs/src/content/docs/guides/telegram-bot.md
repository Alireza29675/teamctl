---
title: Telegram bot setup
---

The `team-bot` binary is the first interface adapter — it connects a Telegram chat to the teamctl mailbox so you can DM managers and approve sensitive actions.

## Prerequisites

1. Create a bot via [@BotFather](https://t.me/BotFather). Save the token.
2. Find your Telegram chat id — send any message to [@userinfobot](https://t.me/userinfobot). It's a number.

## Configuration

```yaml
# team-compose.yaml
interfaces:
  - type: telegram
    name: tg-main
    config:
      bot_token_env: TEAMCTL_TELEGRAM_TOKEN
      authorized_chat_ids: [75473051]
```

Then export the token and start the bot:

```bash
export TEAMCTL_TELEGRAM_TOKEN="123456:AAH…"
export TEAMCTL_TELEGRAM_CHATS="75473051"
team-bot --mailbox ./state/mailbox.db
```

(An interactive `teamctl bot setup` wraps this in one command.)

## What the bot does

- **Forwards manager messages.** Any message addressed to an agent with `telegram_inbox: true` is sent to the authorized chat.
- **Surfaces approvals.** New pending `request_approval` rows appear with Approve / Deny inline buttons.
- **Accepts commands:**
  - `/dm <project>:<agent> <text>` — send a message into the mailbox.
  - `/pending` — list pending approvals.
  - `/help` — this help.

## Security

- Messages from chat ids not in `authorized_chat_ids` are silently dropped and logged.
- Approval callbacks are rejected the same way.
- The bot never calls Telegram from anywhere but a manager-addressed message or an approval — it is not a general-purpose notifier.
