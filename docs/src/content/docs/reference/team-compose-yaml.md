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

interfaces:
  - type: telegram
    name: tg-main
    config:
      bot_token_env: TEAMCTL_TELEGRAM_TOKEN
      authorized_chat_ids: [75473051]
      manager: news:head_editor        # optional — scope bot to one manager

projects:
  - file: projects/newsroom.yaml
  - file: projects/blog-site.yaml
```

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
    telegram_inbox: true
    reports_to_user: true
    autonomy: low_risk_only
    can_dm: [fact_checker, news_writer, seo]
    can_broadcast: [editorial, all]

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
| `interfaces` | list | `[]` | Human-facing channels (telegram, discord, imessage, cli, webhook, email). |
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
| `telegram_inbox` | bool | `false` | Manager-only. Set `true` to receive Telegram forwards. |
| `reports_to_user` | bool | `false` | Manager-only. May call `reply_to_user`. |
| `autonomy` | string | `low_risk_only` | `full` · `low_risk_only` · `proposal_only`. |
| `can_dm` | list | `[]` = unrestricted | Short-names this agent may DM. |
| `can_broadcast` | list | `[]` = unrestricted | Channel names this agent may post to. |
| `reports_to` | string | — | Worker-only. The manager this worker answers to. |
