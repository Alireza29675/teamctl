# Example: startup-team

A small startup — an idealist founder, a product manager, an engineering
lead and an IC, and a researcher. You, the owner, talk to **both
managers through their own Telegram bots**. The managers coordinate
with each other in a private `#leads` channel; the engineers and
researcher work under the PM and founder respectively.

```
┌─ founder (Claude Opus)                  ← Telegram: founder bot
│    └─ researcher (Claude Sonnet)
│
└─ product_manager (Claude Opus)          ← Telegram: product bot
     ├─ eng_lead (Claude Opus)
     └─ eng_ic (Codex GPT-5)
```

## Install

```bash
# 1. Install teamctl + runtimes.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code
# codex — see OpenAI's install docs

# 2. Create two Telegram bots via @BotFather.
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/startup-team ~/startup
cd ~/startup

# 4. Fill in tokens + chat ids.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir.
mkdir -p workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Now start the two bots. They share the same SQLite mailbox but each is
scoped to one manager — the founder bot only forwards traffic for
`founder`, the product bot only forwards for `product_manager`.

```bash
# Founder bot (in its own tmux window / terminal)
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$FOUNDER_BOT_TOKEN" \
  --authorized-chat-ids "$FOUNDER_CHAT_IDS" \
  --manager startup:founder

# Product bot (separate window / terminal)
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$PRODUCT_BOT_TOKEN" \
  --authorized-chat-ids "$PRODUCT_CHAT_IDS" \
  --manager startup:product_manager
```

You'll see two Telegram chats — one per bot. DM the founder bot with
big-picture questions ("what are we really trying to prove?"). DM the
product bot with execution questions ("when can we ship invite links?").

## Shape of a typical day

1. Owner DMs founder bot: *"I want to talk about pricing."*
2. Founder responds. If it turns into a scope decision, founder loops in
   `product_manager` via `#leads`.
3. Owner DMs product bot: *"Add email-based invite links before Friday."*
4. PM writes a one-pager, DMs `eng_lead`, who drafts a plan, who DMs
   `eng_ic`.
5. `eng_ic` ships. `eng_lead` calls `request_approval(action="deploy")`.
   Owner sees the approval request in the product bot and taps ✅.
6. PM reports back: *"Shipped. Link: …"*.

## Teardown

```bash
teamctl down
rm -rf state/
```
