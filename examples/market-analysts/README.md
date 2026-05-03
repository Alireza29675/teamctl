# Example: market-analysts

A small research desk whose only job is to help you make
**backed** financial decisions. Five analysts: a chief who's the only
one that reaches you over Telegram, three specialist analysts
(macro / equities / crypto), and a read-only quant-risk analyst whose
sole job is to dissent.

The desk is **read-only by design**. Any action that could move money
(`trade`, `payment`, etc.) goes through HITL approval — the chief
proposes, you approve or deny on Telegram.

```
chief (Opus) ─── macro     (Opus)
             ├── equities  (Sonnet)
             ├── crypto    (Sonnet)
             └── quant_risk (Opus, plan-mode · read-only)
                  ↳ vetoes everything the others send up
```

## Install

```bash
# 1. Install teamctl and Claude Code.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create a Telegram bot via @BotFather.
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/market-analysts ~/markets
cd ~/markets

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace for notes, transcripts, research files.
mkdir -p workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status

# In a separate terminal: start the Telegram bot, scoped to the chief.
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$TEAMCTL_TG_CHIEF_TOKEN" \
  --authorized-chat-ids "$TEAMCTL_TG_CHIEF_CHATS" \
  --manager markets:chief
```

## How the desk talks to you

- **Proactively**: when an analyst flags something to `#alerts` *and*
  `quant_risk` cross-checks it *and* it has an actionable time horizon,
  the chief DMs you. These are rare. The point is signal over noise.
- **On demand**: DM the bot anything — "what's the read on BTC funding
  right now?", "is the dollar move pre-FOMC overdone?" — and the chief
  will respond, pulling in analysts as needed.
- **Scheduled**: a close-of-session brief to `#desk` daily.

Every message you get from the chief carries **"Not advice —
observation only"**. Every action that would move money shows up as a
separate approval request in `teamctl pending`.

## Example session

```
You (Telegram):  What's the overnight read on 2y yields?

chief:           2y up 8bps on a hawkish Powell re-read (transcript link).
                 2y/10y widened from -18 to -26bps, fastest 24h move in 3m.
                 macro thinks this is an upside-surprise read; quant_risk
                 flags that dealer gamma is negative in the belly, so the
                 move can extend. Confidence: medium. 48h horizon.
                 Not advice — observation only.

You:             What would flip it?

chief:           Williams, 10:00 UTC tomorrow. If he explicitly walks back
                 "higher for longer" the 2y unwinds fast.
```

## Teardown

```bash
teamctl down
rm -rf state/
```
