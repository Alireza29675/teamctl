# Example: market-analysis

A four-agent research desk that watches the markets you care about and tells you when something's worth your attention. Read-only by design — the desk gives you the *read*, not the trade.

```
chief (Claude Opus)              ← Telegram: chief bot
   owns: synthesis, operator-facing call
       │
   ┌───┼────────────────────┐
   │   │                    │
collector       interpreter       risk
(Sonnet)         (Opus)         (Opus, plan-mode)
 owns: data     owns: the         owns: dissent,
 feeds          read & narrative  cross-asset stress
```

The desk is **read-only by design**. Nothing here trades or moves money. `trade`, `payment`, and `external_email` are all globally-sensitive — the desk never even proposes a position. Your job is to read what the desk surfaces and decide whether to act on it; their job is to be a sharper analyst than any single thinker.

## Why four agents earns more than three

The market-analysis desk passes both gates with multiple triggers — that's what makes a four-agent team better than collapsing two of them:

- **Entry conditions** — each agent owns a real domain end-to-end. Data feeds, narrative read, dissent, synthesis. All four have rhythms, compounding state, and reasons to keep going.
- **Work-shape triggers** — *domain separation* and *focus separation* both fire. The data domain is a different surface from the read; the read is a different surface from the dissent; the synthesis is a different surface still.
- **Team-shape triggers** — *multiple opinions* is the entire point. Interpreter's read and risk's dissent are *supposed* to push against each other; the chief's synthesis is only valuable because it integrates the friction. A single agent doing all three jobs would always agree with itself, and the desk would be worse for it.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for chief).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/market-analysis ~/markets
cd ~/markets

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace — where the desk caches data, transcripts, and notes.
mkdir -p workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Or use `teamctl bot setup` for the guided Telegram wizard.

## How the desk talks to you

Three patterns, all through the chief bot on Telegram:

- **Proactively** — rare. When the desk converges on something real (collector flags a notable print, interpreter lands a thesis, risk signs off or escalates), chief DMs you with the compact synthesis. Signal over noise; most days are quiet.
- **On demand** — DM the chief bot with anything: *"what's the read on 2y yields?"*, *"is the EUR move pre-FOMC overdone?"*, *"why did equities reverse yesterday?"*. Chief routes internally and circles back with the integrated answer.
- **Scheduled** — a daily close note to `#alerts` summarising what moved, what didn't, and the desk's working theses.

Every message you get ends with **"Not advice — observation only."** The desk reads the market; you decide what to do with the read.

[Screenshot of a Telegram conversation: user asks "overnight read on 2y yields?" — chief responds with the synthesis paragraph integrating interpreter's read and risk's dissent]

[Screenshot of a proactive alert: chief surfaces a notable move with the desk's view and what would flip it]

## What this teaches

Three patterns layer:

1. **Dissent as a domain.** Risk runs in `permission_mode: plan` and exists to disagree. The desk is stronger because someone is paid to push back on the working thesis. Try collapsing risk into interpreter and you lose the *multiple opinions* trigger that makes the team valuable.
2. **Channel insulation by purpose.** `#desk` is where the analysts argue; `#alerts` is where the operator-shaped output goes. Collector posts data prints to `#desk` only — that conversation doesn't need to flood `#alerts`. Channel design is the team's information architecture.
3. **HITL on anything that moves money.** `trade`, `payment`, and `external_email` are all gated. The desk can't even *propose* a trade without your tap. The read-only-by-design pattern is what makes the desk safe to leave running.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/) — the methodology this example is built on, including the four Gate (b) triggers.
- [Newsletter team](../newsletter-team/) — small 2-agent team where multi-opinions is the main team-shape trigger.
- [SaaS product team](../saas-product/) — large team where domain-cut survives at scale.
