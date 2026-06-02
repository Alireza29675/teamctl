---
title: "Reference: `team-compose.yaml`"
---

The compose tree has two layers:

- One **global** file (`team-compose.yaml`) — broker, supervisor, budget, HITL policy, interfaces, and the list of project files.
- One **per-project** file (`projects/<id>.yaml`) — channels, managers, workers.

## Global

```yaml
version: "2.0.0"

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
version: "2.0.0"

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
    model: claude-opus-4-8
    role_prompt: roles/head_editor.md
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
| `role_prompt` | path or list of paths | — | System prompt source. A single string keeps the current behavior — the file is passed straight to the runtime. A list concatenates the files in declared order (separated by an em-dash) into `state/role_prompts/<project>-<agent>.md` and points the runtime at that file. Re-materialized on every render, so editing any source file flows into the agent's prompt at the next `up` / `reload`. |
| `permission_mode` | string | headless (`--dangerously-skip-permissions`) | Claude Code permission mode. Headless agents launch with `--dangerously-skip-permissions` so an unattended pane never freezes on a permission dialog no human is there to answer — the only mode that reliably keeps a headless agent draining its inbox. `attended` is teamctl-specific — it marks a human-at-keyboard agent, so `teamctl` passes no skip-permissions / `--permission-mode` flag and skips the `PreToolUse` deny hook that blocks synchronous-prompt tools (`AskUserQuestion`, plan mode) on headless agents. An explicit Claude mode (`plan` for a read-only critic, `acceptEdits`, `bypassPermissions`, `dontAsk`) is forwarded as `--permission-mode`. Avoid `auto` for headless agents: it is a classifier mode that still prompts/blocks (and whose first-run trust prompt the auto-confirm watcher cannot satisfy), so it strands unattended panes. |
| `interfaces.telegram` | map | — | Manager-only. 1:1 Telegram bot for this manager (presence implies it receives Telegram forwards and may call `reply_to_user`). |
| `autonomy` | string | `low_risk_only` | `full` · `low_risk_only` · `proposal_only`. |
| `can_dm` | list | `[]` = unrestricted | Short-names this agent may DM. |
| `can_broadcast` | list | `[]` = unrestricted | Channel names this agent may post to. |
| `reports_to` | string | — | Worker-only. The manager this worker answers to. |
| `effort` | string (enum) | wrapper default | Per-agent reasoning effort: `low` · `medium` · `high` · `xhigh` · `max`. Renders as `EFFORT=<value>` and is passed to the runtime (e.g. `claude --effort <value>`). Strict: an unknown value fails validation rather than silently falling back. |
| `on_rate_limit` | list | global default | Overrides the global rate-limit hook chain for this agent. Each entry is the `name` of a hook defined under the global `rate_limits.hooks`. Unset falls back to the global `rate_limits.default_on_hit` chain (which is `[wait]` when empty). |
| `display_name` | string | agent id | Human-friendly label the TUI shows (roster, detail header, mailbox attribution, statusline) in place of the agent id. Non-empty, ≤64 chars. Render-time only: the agent id stays canonical for routing, tmux session names, and YAML cross-refs (`can_dm`, `can_broadcast`, `reports_to`). |
| `hooks` | list | `[]` | Per-agent Claude Code hooks, merged additively into the agent's rendered `settings.json` (the built-in interactive-prompt deny hook keeps precedence). Each entry: `{ event, matcher?, command }`, where `command` is a compose-root-relative path resolved like `role_prompt`. Claude Code agents only; on other runtimes it renders nothing and logs a warning. |
| `mcps` | map | `{}` | Per-agent MCP servers, merged alongside the built-in `team` mailbox server (a declared server named `team` is rejected). Keyed by server name; each value: `{ command, args?, env? }`. Runtime-agnostic: rendered for any runtime whose descriptor sets `supports_mcp`, skipped with a warning otherwise. |
| `subagents` | list | `[]` | Per-agent Claude Code sub-agents. Each entry is a compose-root-relative path to a sub-agent markdown file (frontmatter `name` / `description` / optional `tools` / `model`; body becomes its system prompt). Passed via Claude Code's `--agents`, additively on top of the project `.claude/agents/`. Claude Code agents only; on other runtimes it renders nothing and logs a warning. |
| `skills` | list | `[]` | Per-agent Claude Code skills. Each entry is a compose-root-relative path to a skill directory (the folder holding `SKILL.md`). Materialized into a per-agent scope dir and exposed via `claude --add-dir`, additively on top of the project `.claude/skills/`. Claude Code agents only; on other runtimes it renders nothing and logs a warning. |
| `interfaces.telegram.bot_token_env` | string | — | Env var holding the BotFather token. Populated by `teamctl bot setup`. |
| `interfaces.telegram.chat_ids_env` | string | — | Env var holding a comma-separated allow-list of chat ids. |
