# Example: solo-triage

The team a solo developer wishes they had: a **manager** who fields
"what's on my plate," a **research** worker who chases context across
the web and your project's docs, and an **inbox** worker who watches
your queues, drafts replies, and keeps a running journal of what
happened today. You — the solo dev — talk to one bot and stay in the
work that only you can do.

```
manager (Claude Opus)              ← Telegram: manager bot
  ├─ research  (Claude Sonnet)     · #research — chases context
  └─ inbox     (Claude Sonnet)     · #inbox    — drafts replies + journals
```

Your `manager` is your mission control. They route requests to the two
workers, hold a coherent picture of what's happening, and ask before
sending anything to a real human. `research` and `inbox` are insulated
from each other's channels — research notes don't bleed into the inbox
queue, and the inbox draft pile doesn't flood research. Both report up
to the manager, and the manager talks to you.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather.
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/solo-triage ~/triage
cd ~/triage

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir (where the agents read your project from).
mkdir -p workspace
# Tip: symlink your repo and any inbox sources into ./workspace/ so
# the agents can read them.
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Now start the manager bot:

```bash
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$TEAMCTL_TG_MANAGER_TOKEN" \
  --authorized-chat-ids "$TEAMCTL_TG_MANAGER_CHATS" \
  --manager triage:manager
```

DM the manager bot when something lands on your plate — paste the
GitHub issue, the email thread, or the half-formed thought, and let
the manager route it. Drafts that are about to leave your inbox come
back as Telegram approval prompts; you read them on your phone, tap
✅, and the inbox worker sends them.

## What this demonstrates

A **solo-dev mission-control workflow with HITL on outbound writes**.
Three patterns layer:

1. **Hub-and-spoke.** The manager is the only agent that talks to you,
   and the only agent that talks to both workers. research and inbox
   never need to coordinate directly — the manager holds the thread.
2. **Channel insulation.** `#research`'s context-chase notes and
   `#inbox`'s draft pile live in different channels. Neither worker
   sees the other's ongoing chatter; the manager sees both.
3. **HITL on outbound writes.** `external_email` and `publish` are on
   the globally-sensitive list. Anything `inbox` would send to a real
   human, or anything that would post under your name in public,
   pauses for a Telegram approval. The tap is the audit trail.

The manager's role prompt is the load-bearing piece — it's what makes
mission-control useful instead of noisy. It lives in
`.team/roles/manager.md`.

## Shape of a typical day

1. Something lands — a GitHub issue, an email, a vague idea you DM the
   manager bot. The manager reads it, decides whether it needs context
   or a reply, and routes.
2. `research` chases context: reads the linked docs, the related code,
   the prior threads. Drops a 3-5 bullet brief in `#research`.
3. `inbox` drafts the reply (or the journal entry). Posts the draft in
   `#inbox` and calls `request_approval(action="external_email", ...)`
   if it's outbound. You tap ✅ on Telegram; it sends.
4. End of day, `inbox` writes a one-paragraph journal entry — what
   landed, what was answered, what's still open — and posts it to
   `#all`. You skim it before bed.

## Teardown

```bash
teamctl down
rm -rf state/
```
