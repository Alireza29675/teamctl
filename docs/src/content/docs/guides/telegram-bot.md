---
title: Telegram bot setup
---

Each user-facing manager gets its own Telegram bot. You DM the manager's
bot in plain English; the message is routed straight to that manager's
mailbox. Approvals and replies come back through the same chat.

`teamctl bot setup` is the one command that wires everything: token,
authorization, env vars, and the per-manager YAML block.

## Prerequisites

- `curl` on PATH (used during setup to call the Telegram API).
- A Telegram account, ready to DM [@BotFather](https://t.me/BotFather).
- At least one manager declared in `projects/<id>.yaml`. The wizard
  enumerates every manager and lets you pick which to wire up.

## Run the wizard

```bash
teamctl bot setup
```

For each user-facing manager, the wizard walks you through:

1. **Create a bot.** Open BotFather, send `/newbot`, follow prompts.
   BotFather returns a token like `123456:AAH-…`. Paste it.
2. **Verify the token.** The wizard hits `getMe` and shows the bot's
   resolved username so you know it's the right one.
3. **Authorize your chat.** The wizard prints "Send `/start` to
   @your-bot." It long-polls for the next `/start` and captures your
   chat id automatically.
4. **Pick env-var names.** Defaults are `TEAMCTL_TG_<MANAGER>_TOKEN`
   and `TEAMCTL_TG_<MANAGER>_CHATS`; press Enter to accept or type
   your own.

The wizard then:

- Writes both values into `.team/.env` (creates it if missing,
  upserts in place if present — your other vars are preserved).
- Adds an `interfaces.telegram` block to that manager in
  `projects/<id>.yaml`. Sibling adapters (`discord:` etc.) are
  preserved on a re-run.

Re-run `teamctl bot setup` any time. The wizard is **resumable**:

- Fully-configured managers are skipped silently.
- If the YAML already has env-var names, they're reused — you're not
  asked to pick names again.
- If only the token or only the chat-id is set in `.env`, the wizard
  collects just the missing piece.
- `--force` re-asks for everything.

Scope to one manager by passing it as a positional argument:

```bash
teamctl bot setup news:head_editor
```

## Launch

```bash
teamctl up
```

`teamctl up` now also spawns a `team-bot` tmux session per manager
that has an `interfaces.telegram` block. Open Telegram, find your
bot, and start typing — every message flows into that manager's
mailbox.

Useful commands:

- `teamctl bot list` — show every configured manager + env-var status.
- `teamctl bot status` — show which bot tmux sessions are running.
- `teamctl down` — stops bots alongside agents.

## What you can send

- **Plain text** — routed as a DM to the bot's manager. No `/dm`
  prefix needed.
- **`/pending`** — list pending approvals.
- **Approve / Deny inline buttons** — appear under each approval
  request the manager raises. One tap resolves it.
- **`/dm <project>:<agent> <text>`** — escape hatch for talking to a
  different agent through the same bot. Useful in a pinch; not the
  daily-driver path.
- **`/clear`, `/compact`, `/help`, …** — any slash command not
  recognised by the bot above is **typed straight into the manager's
  tmux session** (Claude Code only). So `/clear` from Telegram clears
  the manager's conversation, `/compact` summarises it, and so on —
  whatever the runtime exposes. Non-Claude-Code managers reply with a
  feature-gate message naming the actual runtime.

## YAML it produces

```yaml
# projects/news.yaml
managers:
  head_editor:
    runtime: claude-code
    role_prompt: roles/head_editor.md
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_HEAD_EDITOR_TOKEN
        chat_ids_env: TEAMCTL_TG_HEAD_EDITOR_CHATS
```

```bash
# .team/.env  (gitignored)
TEAMCTL_TG_HEAD_EDITOR_TOKEN=123456:AAH-...
TEAMCTL_TG_HEAD_EDITOR_CHATS=75473051
```

## Security

- Messages from chat ids not in `chat_ids_env` are silently dropped.
- Approval callbacks go through the same authorization gate.
- `.env` is in the shipped `.gitignore`. Never commit tokens.
- One bot per manager keeps blast radius minimal: a leaked PM token
  doesn't expose eng_lead's approvals.
- **Slash-passthrough trust posture**: the bot is per-manager and
  chat-id-gated, so the operator owns the bot end-to-end. Slash
  commands typed via Telegram run at the agent's privilege — the
  same trust boundary the operator already extends via direct
  `tmux attach` or `ssh` to the host. There is no allowlist on
  slash content; if you wouldn't paste it into the agent's tmux
  session yourself, don't paste it into Telegram either. Multi-user
  shared bots are out of scope; if a future deployment needs them,
  the slash-passthrough surface gains an allowlist.
