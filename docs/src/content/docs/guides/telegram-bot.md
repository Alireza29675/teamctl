---
title: Named Telegram interfaces
---

Telegram is optional. The browser works automatically without it. Create a bot with Telegram's BotFather, authorize your own chat IDs, and store token/chat-ID values in the ignored `.team/.env`. Declare references in `.team/team-compose.yaml`:

```yaml
interfaces:
  - id: private-phone
    provider: telegram
    name: Private phone
    description: Private notes and everyday requests
    config:
      agent: personal:companion
      bot_token_env: PRIVATE_BOT_TOKEN
      authorized_chat_ids_env: PRIVATE_CHAT_IDS
```

Use `teamctl reload` after changes. A second bot can target the same agent with another ID, name, description and environment references. See the [personal recipe](https://github.com/Alireza29675/teamctl/tree/main/examples/personal-agent) for a two-bot configuration.

Incoming messages carry provider, ID, name, description and message. Replies inherit the originating conversation. Agents discover grants with `list_interfaces` and select `interface_id` for proactive replies. A bot needs an established authorized conversation first. Revoked or reassigned grants are rechecked when queued work is claimed and immediately before delivery.

Managed launchers pass secret environment-variable names in arguments. The adapter resolves them from the compose root's `.env`, then its parent's `.env`, then inherited environment. Values are parsed literally and never executed as shell code. A restarted bot can therefore pick up token rotation despite a stale tmux environment.

Text and cached image/file replies are supported. Voice transcription needs an explicitly configured speech service with its own authentication. Native agents use ordinary messages for input; terminal-only provider slash commands explain that an interactive tmux agent is required. Native help lists available commands.

Bot transports currently use tmux supervision even when the target agent is native. Agents and adapters have separate lifecycles. Ordinary tests use fake HTTP endpoints and never real Telegram accounts or messages.
