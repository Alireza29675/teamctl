---
title: Interfaces
description: Follow conversations and respond to agents from Telegram or the host.
---

Telegram is the shipping messaging adapter. Each configured user-facing manager gets its own bot: a message to that bot goes to the matching manager's mailbox, and replies and approval requests return through the same chat.

On the host, use `teamctl send`, the terminal UI and the approval commands to follow or direct the work. Discord, iMessage, CLI chat, webhook and email adapters are planned; accepting an interface declaration in the schema does not start a working adapter.

## Connect Telegram

Run `teamctl bot setup`. It writes the credentials to `.team/.env` and adds a Telegram block to the manager:

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

`teamctl up` starts one `team-bot` process per configured manager. Authorized chat IDs can send text, receive replies and decide approval requests. See the [Telegram setup guide](/guides/telegram-bot/).

## Use the host controls

| Control | Purpose |
| --- | --- |
| `teamctl send <project>:<agent> "<text>"` | Send a message to a named agent. |
| `teamctl ui` | Inspect sessions, messages and approval requests. |
| `teamctl pending` | List pending approval requests. |
| `teamctl approve <id>` / `teamctl deny <id>` | Decide a pending request. |

## Adapter contract

An adapter connects a human surface to the mailbox. It reads messages for its subscribed managers, displays them, writes authorized replies and presents approval requests for a decision.

The shipped Telegram adapter is the `team-bot` binary. The top-level `interfaces:` array remains in the schema for other adapters; Telegram's current configuration lives on the manager.

## Related

- [HITL](/concepts/hitl/)
- [Guide: Telegram bot](/guides/telegram-bot/)
