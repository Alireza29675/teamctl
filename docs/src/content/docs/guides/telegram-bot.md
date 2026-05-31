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

The wizard opens with a fork:

1. **Managed bots** — you set up **one** manager bot, and it spawns a
   child bot per manager for you (no per-manager BotFather trips). Needs
   a manager bot with Telegram's [Managed Bots](#managed-bots) capability
   enabled. See [Managed bots](#managed-bots) below.
2. **Manual token** — the original flow: you paste a BotFather token for
   each manager yourself. Documented just below.

Targeting a single manager (`teamctl bot setup news:head_editor`) always
uses the manual path. The rest of this section covers the manual flow.

For each user-facing manager, the manual wizard walks you through:

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

## Managed bots

Instead of creating a bot per manager by hand, you set up **one** manager
bot with Telegram's Managed Bots capability (Bot API 9.6) and let it spawn
the per-manager child bots for you. Each child still ends up as its own
1:1 bot — same end state as the manual flow, fewer BotFather trips.

### Prerequisites

- A manager bot created in [@BotFather](https://t.me/BotFather) with
  **Managed Bots enabled**: `/mybots` → your bot → *Bot Settings* →
  *Managed Bots*.
- `curl` on PATH (token verification), and a Telegram account ready to
  tap the bot-creation links the wizard prints.
- At least one manager declared in `projects/<id>.yaml`.

### Run it

```bash
teamctl bot setup
```

Pick **Managed bots** at the fork, then:

1. **Paste the manager bot token.** The wizard verifies it with `getMe`
   and shows the resolved `@username`.
2. **Confirm each child bot.** For every manager, the wizard prints a
   `t.me` bot-creation link. Open it in Telegram and confirm — the
   manager bot mints a child bot, and the wizard pulls that child's
   token automatically (no copy-paste).
3. **Authorize your chat per child.** Just like the manual flow, the
   wizard asks you to send `/start` to each freshly-minted child bot and
   captures your chat id — so managed mode reaches the same end-state as
   manual (token **and** authorized chat id).
4. The token and chat id for each manager are written into that
   manager's `bot_token_env` / `chat_ids_env` in `.env`, exactly where
   the per-manager bots read them.

### What it writes

A project-level `interfaces.telegram.manager_bot` block records which env
var holds the manager bot token:

```yaml
# projects/news.yaml
interfaces:
  telegram:
    manager_bot:
      token_env: TEAMCTL_TG_MANAGER_TOKEN
managers:
  head_editor:
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_HEAD_EDITOR_TOKEN   # child token, minted for you
        chat_ids_env: TEAMCTL_TG_HEAD_EDITOR_CHATS
```

```bash
# .team/.env  (gitignored)
TEAMCTL_TG_MANAGER_TOKEN=123456:AAH-...        # the manager bot
TEAMCTL_TG_HEAD_EDITOR_TOKEN=789012:AAH-...    # minted child bot
```

### Verify

```bash
teamctl bot list       # every manager + token/chat env-var status
teamctl validate       # the written YAML parses + validates clean
```

### Troubleshooting

- **"Managed Bots not enabled"** — the manager bot token works but can't
  mint children. Enable it in BotFather (*Bot Settings → Managed Bots*)
  and re-run.
- **Link never confirmed** — the wizard waits for you to tap the
  `t.me` link and approve. If it times out, re-run `teamctl bot setup`;
  already-minted children are reused, so you only finish what's missing.
- **Re-running** — `--force` re-collects the manager token and re-mints.
  Without it, configured managers are left as-is.

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
- **Slash-command autocomplete** — manager-scoped Claude Code bots
  register a curated set of slash commands with Telegram on
  startup, so typing `/` in the chat surfaces a menu (`/clear`,
  `/compact`, `/cost`, `/help`, `/init`, `/mcp`, `/model`,
  `/permissions`, `/resume`, `/review`, `/status`, `/vim`).
  Hyphenated commands (`/output-style`, `/pr-comments`,
  `/release-notes`, `/security-review`) aren't in the menu —
  Telegram's bot-API restricts command names to `[a-z0-9_]` — but
  you can still type them manually. Non-Claude-Code manager bots
  register nothing.

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

## Voice messages (optional)

Add a `speech_to_text:` block under a manager's `telegram:` config and
voice notes sent to that bot get transcribed and forwarded to the
agent. The bot replies with `🎙 "<transcript>"` so you can verify what
was heard, and the agent receives the transcript prefixed with
`🎙 (transcribed voice, may have misspellings):`. Silence and
unrecognizable audio yield a friendly "couldn't capture anything"
reply with no inbox disturbance.

```yaml
managers:
  head_editor:
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_HEAD_EDITOR_TOKEN
        chat_ids_env: TEAMCTL_TG_HEAD_EDITOR_CHATS
        speech_to_text:
          provider: groq
          api_key_env: GROQ_API_KEY
          model: whisper-large-v3
          # language: en   # optional ISO-639 hint
```

```bash
# .team/.env  (gitignored)
GROQ_API_KEY=gsk_...
```

Drop the `speech_to_text:` block (or unset the API key env var) to
disable. v1 supports Groq Whisper; other providers (OpenAI Whisper,
whisper.cpp) are intended as one-line additions when needed.

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
