---
title: ADR 0005 — Per-manager Telegram bots and `teamctl bot setup`
---

- Status: **accepted** (implemented in v0.6.0)
- Date: 2026-05-02
- Author: Alireza
- Reviewers:

## Context

The shipping Telegram adapter (v0.2.0 → v0.5.1) is one global bot per
team. To talk to a manager you DM the bot with `/dm <project>:<role>
<text>`. To set it up you hand-write a top-level `interfaces:` block
in `team-compose.yaml`, paste a token into `.env`, find your chat id
via `@userinfobot` (or run the bot once and read the `/start` echo
introduced by v0.2.9), and start `team-bot` yourself.

Three friction points keep biting:

1. **`/dm role text` is not "messaging".** It feels like an IRC
   incantation. Operators forget the role name, mistype the project
   prefix, and cannot use Telegram's drafts or reply threads in a
   meaningful way.
2. **Setup is a documentation walk.** The Telegram guide is six steps
   spread across BotFather, `.env`, YAML, and a separate `team-bot`
   process. New users either skip it or get stuck partway.
3. **One bot is a routing puzzle.** With three managers (PM, eng_lead,
   marketing) you see every approval and every reply on the same
   thread. The v0.2.7 per-manager scoping (`--manager`) helped, but
   only if you ran multiple bots — and there was no command to set
   any of them up.

## Decision

1. **One Telegram bot per user-facing manager.** Each manager that
   sets `reports_to_user: true` carries its own `interfaces.telegram`
   block on the manager definition itself — *not* in the top-level
   `interfaces:` array. The presence of the block is what signals
   "this manager receives Telegram forwards", so the legacy
   `telegram_inbox: true` flag is gone.

   ```yaml
   managers:
     pm:
       runtime: claude-code
       reports_to_user: true
       interfaces:
         telegram:
           bot_token_env: TEAMCTL_TG_PM_TOKEN
           chat_ids_env: TEAMCTL_TG_PM_CHATS
   ```

2. **`teamctl bot setup` is the wizard.** It enumerates user-facing
   managers, walks BotFather → token → `/start` → chat id for each,
   prompts for env-var names (with sensible defaults), persists the
   values into `.team/.env`, and upserts the `interfaces.telegram`
   block into `projects/<id>.yaml`.

3. **`teamctl up` spawns one `team-bot` per manager-with-`interfaces.telegram`.**
   Each runs in its own tmux session named `<prefix>bot-<project>-<role>`
   with `--manager <project>:<role>` so approvals and replies route
   to exactly that bot. `teamctl down` stops them.

4. **DMing the bot Just Works.** `team-bot` interprets any plain text
   on a manager-scoped bot as a message to that manager (not as an
   IRC command). `/dm`, `/pending`, and inline approval buttons stay
   as escape hatches.

5. **The top-level `interfaces:` array stays for non-Telegram
   adapters** (Discord, iMessage, CLI, webhook) since those don't fit
   the 1:1-with-manager model as cleanly. Telegram migrates off it.

## Why not …

- **One bot, smarter routing?** Tried (v0.2.0–v0.5.1). Operators read
  the resulting thread as a firehose; reply context is lost; per-bot
  approval routing is what the v0.2.7 `--manager` scoping was already
  reaching for. Per-bot is the cleaner shape — let Telegram's chat
  metaphor do the routing instead of inventing one inside the bot.

- **Keep the top-level `interfaces:` array and just teach the wizard
  to write to it?** Tried during the spike. The top-level array is one
  level removed from the manager it serves; you have to re-derive the
  relationship every time you read the YAML. Putting
  `interfaces.telegram` directly on the manager keeps related fields
  together (`reports_to_user`, the per-manager interfaces map) and
  removes a YAML cross-reference. The `interfaces:` *map* on the
  manager (rather than a bare `telegram:` key) leaves room for
  `discord:` / `imessage:` siblings without another schema bump.

- **HTTP client dependency for the wizard?** Avoided. Setup shells
  out to `curl` for `getMe` and `getUpdates`. `curl` ships with macOS
  and is on every Linux distro we support, so we keep teamctl's dep
  tree small and don't pull a TLS stack into the CLI for one
  interactive code path.

## Consequences

- **Migration**: existing teams with a top-level `interfaces:`
  Telegram entry keep working as long as they keep starting `team-bot`
  themselves — the schema accepts the legacy block (it lives under
  `Global.interfaces` for non-Telegram adapters anyway). The wizard
  ignores it and writes to the new `interfaces.telegram` shape. The
  `telegram_inbox: true` field is removed; presence of
  `interfaces.telegram` is the new signal. Examples and the dogfood
  team move to the new shape in 0.6.0.
- **Cost**: one Telegram bot per manager. BotFather is free and
  unlimited; each bot is one tmux session and one teloxide
  long-poll. Resource impact is negligible.
- **Security**: every bot has its own token and its own allow-list.
  A leaked PM token does not expose eng_lead's approvals.
