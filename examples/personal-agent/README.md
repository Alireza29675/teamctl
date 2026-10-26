# Personal agent

One persistent agent with a private notebook, task list and a bounded work loop. The web workspace works without Telegram or an API key for teamctl; your chosen model CLI needs its own authenticated account.

```sh
cd examples/personal-agent
teamctl validate
teamctl up
teamctl serve
```

`up` runs Companion in the background; `serve` opens the messenger and inspection tools. Closing the page keeps Companion running. `teamctl down` stops the agent. Notes remain in `workspace/`; messages/assets remain in `.team/state/`.

The default is Codex using its configured/default model. Authenticate with `codex login` first. To use Claude Code or OpenCode, change `runtime` in `.team/projects/personal.yaml` to `claude-code` or `opencode` and authenticate that CLI. Set `model` explicitly only when your account supports it. Runtime sessions resume automatically. Every fifteen minutes Companion reviews its task list for one bounded turn; change `wake_interval_seconds` or remove it for message-driven work only.

## Telegram, including two bots

Create bots with BotFather, copy `.team/.env.example` to `.team/.env`, and fill the token and allowed chat ID variables. Add to `.team/team-compose.yaml`:

```yaml
interfaces:
  - id: personal-phone
    provider: telegram
    name: Personal phone
    description: Private notes and everyday requests
    config:
      agent: personal:companion
      bot_token_env: PERSONAL_TG_TOKEN
      authorized_chat_ids_env: PERSONAL_TG_CHATS
  - id: work-phone
    provider: telegram
    name: Work phone
    description: Work topics and project updates
    config:
      agent: personal:companion
      bot_token_env: WORK_TG_TOKEN
      authorized_chat_ids_env: WORK_TG_CHATS
```

Then `teamctl reload`. Each bot is an independent interface, with its own identity, allowed chats and reply route. The agent discovers both with `list_interfaces`; incoming messages carry their provider, name, description and ID. Web remains available automatically. Secrets stay in the ignored .env; the manifest holds only environment variable names.

## MCP connections and isolation

Per-agent `mcps` assigns additional tools. See the [schema guide](../../docs/src/content/docs/reference/team-compose-yaml.md) and [native/sandbox guide](../../docs/src/content/docs/guides/native-agents.md) for environment references, provider authentication, mounted volumes and Docker. The default host mode uses the account running teamctl. Choose Docker explicitly when you want container isolation; a running Docker daemon and a provider-enabled image are prerequisites.
