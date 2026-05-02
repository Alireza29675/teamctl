---
title: "Reference: `team-compose.yaml`"
---

The compose tree has two layers:

- One **global** file (`team-compose.yaml`) — broker, supervisor, budget, HITL policy, interfaces, and the list of project files.
- One **per-project** file (`projects/<id>.yaml`) — channels, managers, workers.

## Global

```yaml
version: 2

broker:
  type: sqlite                        # sqlite (default); redis-streams is planned
  path: state/mailbox.db

supervisor:
  type: tmux                          # tmux (default) | systemd | launchd
  tmux_prefix: a-                     # tmux session name prefix

budget:
  daily_usd_limit: 25.0
  warn_threshold_pct: 75
  message_ttl_hours: 24
  per_project_usd_limit:
    newsroom: 15.0

hitl:
  globally_sensitive_actions:
    - publish
    - release
    - deploy
    - payment
    - external_email
    - external_api_post
    - merge_to_main
    - dns_change
  auto_approve_windows:
    - action: publish
      project: newsroom
      scope: "morning-brief-*"
      until: "2026-05-01T09:00:00Z"

projects:
  - file: projects/newsroom.yaml
  - file: projects/blog-site.yaml
```

> Telegram bots live on the **manager** definition itself — see the
> per-project example below — and are configured via `teamctl bot setup`,
> which writes both the env vars and the `telegram:` block. The
> top-level `interfaces:` array is no longer needed for Telegram.

## Per-project

```yaml
version: 2

project:
  id: newsroom
  name: Newsroom
  cwd: .

channels:
  - name: editorial
    members: [head_editor, fact_checker, news_writer]
  - name: all
    members: "*"

managers:
  head_editor:
    runtime: claude-code
    model: claude-opus-4-7
    role_prompt: roles/head_editor.md
    permission_mode: auto
    autonomy: low_risk_only
    can_dm: [fact_checker, news_writer, seo]
    can_broadcast: [editorial, all]
    # Per-manager 1:1 Telegram bot. Run `teamctl bot setup` to populate
    # both the env vars and this block. After setup, `teamctl up`
    # spawns one team-bot per manager and DMing the bot reaches the
    # manager directly — no `/dm role text` needed.
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_HEAD_EDITOR_TOKEN
        chat_ids_env: TEAMCTL_TG_HEAD_EDITOR_CHATS

workers:
  fact_checker:
    runtime: gemini
    model: gemini-3.0-pro
    role_prompt: roles/fact_checker.md
    reports_to: head_editor
    can_dm: [head_editor, news_writer]
    can_broadcast: [editorial]

  news_writer:
    runtime: claude-code
    model: claude-sonnet-4-6
    role_prompt: roles/news_writer.md
    reports_to: head_editor
    can_dm: [head_editor, fact_checker]
    can_broadcast: [editorial]
```

## Field reference

### Global

| Field | Type | Default | Notes |
|---|---|---|---|
| `version` | int | — | Must be `2`. |
| `broker.type` | string | `sqlite` | Only `sqlite` is shipping. |
| `broker.path` | string | `state/mailbox.db` | Resolved relative to the compose root. |
| `supervisor.type` | string | `tmux` | `tmux` · `systemd` · `launchd`. |
| `supervisor.tmux_prefix` | string | `a-` | Tmux session name = `<prefix><project>-<agent>`. |
| `budget.daily_usd_limit` | float | — | Overall ceiling. |
| `budget.per_project_usd_limit` | map | `{}` | Per-project overrides. |
| `budget.message_ttl_hours` | int | 24 | `teamctl gc` horizon. |
| `hitl.globally_sensitive_actions` | list | (see default) | Actions that always gate through approval. |
| `hitl.auto_approve_windows` | list | `[]` | Pre-authorization windows. |
| `interfaces` | list | `[]` | Reserved for non-Telegram adapters (discord, imessage, cli, webhook, email). Telegram now lives on the manager. |
| `projects` | list | `[]` | Each entry: `{ file: <path> }`. |

### Per-project

| Field | Type | Default | Notes |
|---|---|---|---|
| `version` | int | — | Must be `2`. |
| `project.id` | string | — | Unique id; used in tmux session names, mailbox scoping. |
| `project.name` | string | — | Human label. |
| `project.cwd` | path | — | Working directory for runtimes. Relative paths resolve against the compose root. |
| `channels[].name` | string | — | Channel name (project-scoped). |
| `channels[].members` | list or `"*"` | — | Agent short-names or `"*"` for every agent in this project. |
| `managers` / `workers` | map | — | Keyed by agent short-name. |

### Agent

| Field | Type | Default | Notes |
|---|---|---|---|
| `runtime` | string | `claude-code` | Must match a `runtimes/<name>.yaml`. |
| `model` | string | runtime default | Runtime-specific model id. |
| `role_prompt` | path | — | System prompt file; passed via runtime-specific flag. |
| `permission_mode` | string | runtime default | e.g. `auto`, `plan`, `acceptAll`. |
| `interfaces.telegram` | map | — | Manager-only. 1:1 Telegram bot for this manager (presence implies it receives Telegram forwards and may call `reply_to_user`). |
| `autonomy` | string | `low_risk_only` | `full` · `low_risk_only` · `proposal_only`. |
| `can_dm` | list | `[]` = unrestricted | Short-names this agent may DM. |
| `can_broadcast` | list | `[]` = unrestricted | Channel names this agent may post to. |
| `reports_to` | string | — | Worker-only. The manager this worker answers to. |
| `interfaces.telegram.bot_token_env` | string | — | Env var holding the BotFather token. Populated by `teamctl bot setup`. |
| `interfaces.telegram.chat_ids_env` | string | — | Env var holding a comma-separated allow-list of chat ids. |
