# Example: personal-finance

A three-agent team that holds the picture of your money so you don't have to. **Books** talks to you on Telegram and decides what's worth your attention. **Tracker** watches your accounts and surfaces anomalies. **Analyst** builds the long-arc patterns (weekly / monthly digests, savings rate, category trends). The team is read-only by design: nothing moves money without your tap.

```
books (Claude Opus)              ← Telegram: books bot
   owns: operator-facing financial picture, the "what matters" filter
       │
   ┌───┴──────┐
   │          │
tracker     analyst
(Sonnet)    (Opus)
 owns:       owns:
 live data,  patterns,
 anomaly     savings rate,
 detection   digests
```

You give books your priorities once. *"I care about my savings rate, my emergency fund weeks-of-runway, and not silently overspending on subscriptions."* The team starts watching. When something matters (an anomaly, a developing pattern, a milestone), books DMs you with the specific facts and offers depth. Most days no message lands. That's the right output.

## Why three agents earns more than one

The team passes both gates with all triggers active:

- **Entry conditions.** Books owns the operator-facing picture end-to-end (state: stated priorities, what's been surfaced, what got skipped, voice calibration). Tracker owns the live data domain (state: account roster, freshness baseline, anomaly threshold tuned to the operator's patterns). Analyst owns the synthesis domain (state: category model, trend window, savings-rate history).
- **Work-shape triggers.** *Domain separation*: live data, long-arc patterns, and operator-facing decisions are three different surfaces with three different states. *Focus separation* fires for tracker (continuous attention to refreshes and anomalies, not fired-off per question).
- **Team-shape trigger.** *Synergy*: tracker's anomaly threshold sharpens from analyst's pattern recognition over time; analyst's trends inform what tracker flags as worth surfacing; books's voice calibration teaches both peers what the operator cares about. Compounding three ways.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for books).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/personal-finance ~/finance-team
cd ~/finance-team

# 4. Fill in token + chat id. If you're wiring up a financial-data
#    aggregator (Plaid, Teller, manual CSV), add those credentials too.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir. Where the tracker caches account data and the
#    analyst keeps trend history.
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

## What you do with them

Tell books what you care about. *"My priorities: savings rate above 20%, emergency fund at 6 months expenses, no surprise subscriptions, alert me on transactions over $300."*

The team starts watching. Anomalies surface from tracker; patterns develop in analyst; books decides what earns Telegram. Examples of what books might send you:

> *Anomaly worth a look: $487 at Hardware Store Z on Tuesday. Your average transaction in that account is $40; this is 12x. No previous transactions there in 6 months. Was this you?*

> *October digest: savings rate held at 22%, just below your 20% floor. Dining-out spent 40% above last month, mostly from one week. Holdings: tech allocation now 47%, drifting from your 40% target. Want a deeper look at any of these?*

> *Subscriptions watch: three new recurring charges added this quarter (Service A $12/mo, Service B $9/mo, Service C $24/mo). Cumulative monthly subscription spend is up $45 since July.*

## What this teaches

Three patterns layer:

1. **Read-only by design.** `payment` is on the globally-sensitive list. The team can see your money but cannot move it. Even proposing specific trades or transfers is out of scope. You get the picture; you decide the action.
2. **The signal/noise problem is a domain split.** Tracker filters transactions against the anomaly threshold. Analyst filters patterns against the operator's stated priorities. Books filters everything against *what's worth a Telegram*. Three filters land less noise than one.
3. **Compounding state is the point.** A single transaction has no context. A transaction-against-this-operator's-six-month-pattern has a lot of context. Persistent agents accumulate the context that makes the read informed; sub-agents can't hold this kind of memory.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on.
- [Personal research](../personal-research/). Two-agent personal-information loop with a similar tracker-plus-synthesiser pattern.
