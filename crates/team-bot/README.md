# TeamCTL for Telegram

Follow your TeamCTL team from Telegram. `team-bot` connects authorized chats to a manager's mailbox, carries replies and presents agent-requested approval decisions.

TeamCTL is an open-source platform for autonomous teams of agents. This binary is its shipping messaging adapter.

```bash
cargo install team-bot
teamctl bot setup
teamctl up
```

Run setup inside a configured team. It connects each selected manager to a bot and writes token and chat-ID references into the team definition. `teamctl up` starts the configured bots; avoid starting a second poller with the same token.

Each manager has its own bot and authorized chat IDs. Keep tokens in `.team/.env`, outside shared configuration and source control.

[Telegram setup](https://teamctl.run/guides/telegram-bot/) · [Interfaces](https://teamctl.run/concepts/interfaces/) · [Source](https://github.com/Alireza29675/teamctl)
