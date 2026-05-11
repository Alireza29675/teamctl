# Example: solo-founder-ops

A four-agent team for solo founders. **Hub** holds the day picture and talks to you on Telegram. **Research** chases context on whatever you need to think clearly about. **Inbox** drafts replies and keeps the journal of what landed and what shipped. **Analytics** watches your product metrics and surfaces anomalies. The whole point: you keep your attention on building; the team holds everything else.

```
                       hub (Claude Opus)              ← Telegram: hub bot
                          owns: day picture, cross-worker routing
                                  │
       ┌───────────────────┬─────┴─────┬───────────────────┐
       │                   │           │                   │
  ┌────▼─────┐       ┌─────▼─────┐ ┌──▼──────┐        ┌────▼──────┐
  │ research │       │   inbox   │ │ analytics│        │           │
  │ (Sonnet) │       │ (Sonnet)  │ │  (Opus)  │        │           │
  └──────────┘       └───────────┘ └──────────┘        │           │
   owns:              owns:         owns:               
   briefs,            queue of      product metrics,    
   competitor         asks,         anomaly             
   moves,             drafts,       detection,          
   hire pipelines     daily         weekly digests      
                      journal
```

Two channels keep the information architecture clean: `#ops` (hub + research + inbox; the operational thread) and `#signals` (hub + analytics; the metrics thread). Hub sits on both. The founder sees curated traffic from hub on Telegram; the workers handle the routine traffic between them on the team channels.

## Why four agents earns more than one

A solo founder is the most-constrained operator in startup land. The team passes both gates with multiple triggers active:

- **Entry conditions.** Each agent owns a real domain end-to-end. Hub owns the operational picture (state: founder's stated priorities, what's escalated, what's been answered). Research owns the briefs surface (state: recurring topics, longitudinal competitor pictures, source quality). Inbox owns the asks-queue and journal (state: customer histories, voice profile, what was sent). Analytics owns the metrics surface (state: metric definitions, anomaly thresholds, trend windows).
- **Work-shape triggers.** *Domain separation*: research, asks-handling, metrics-watching, and operator-facing decisions are four different surfaces. *Focus separation*: analytics needs continuous attention to metric pulls; inbox needs continuous attention to incoming asks; both are unfit for fired-off-per-question.
- **Team-shape trigger.** *Synergy*: research findings inform how analytics frames metric anomalies (a competitor's pricing change explains a conversion drop); inbox's journal informs research's recurring-topic tracking (which customers keep coming back); hub's calibration teaches all three what the founder weighs heavily. Compounding across all four.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for hub).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/solo-founder-ops ~/founder-ops
cd ~/founder-ops

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir. Where research caches its briefs, inbox the journal,
#    analytics the metric definitions and pulls.
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

First conversation with hub: tell them about you, your product, what you care about. Be specific.

> *"I'm building \[product\]. Solo for now, may hire in 2-3 months. Metrics I care about: weekly MRR, trial-to-paid conversion, churn. The 3 competitors I want research watching: A, B, C. Customer support comes through Crisp (forwarded to support@). My voice is warm-direct, not formal. I want one queue check from you in the morning, one mid-afternoon, and otherwise only ping me on urgencies."*

Hub briefs the team. Research starts watching the three competitors. Inbox starts watching the support email. Analytics starts pulling your product metrics. Within a day, the rhythm establishes itself.

Examples of what hub might surface on Telegram:

> *Morning check: 3 things waiting on you. 1) Reply approval for Customer X (drafted, warm-direct, 5 sentences). 2) Competitor B raised a Series B; research brief queued for whenever you have 10 minutes. 3) Signal: conversion dropped from 14% to 9.5% last week, anomaly investigation underway.*

> *Urgent: Customer Y posted publicly about a bug. 800 followers, gaining traction. Drafted apology + acknowledgment. Approve and send?*

> *End of day: 7 customer replies sent today, 1 escalated to you and resolved, 2 still in your queue. MRR up $200 today. Research found nothing material on the competitor watch.*

## What this teaches

Three patterns layer:

1. **Hub-and-spoke beats federation for solo operators.** The hub holds the cross-worker view so the workers don't have to coordinate amongst themselves. Each worker DMs hub; hub routes; the founder sees one Telegram thread, not four. This shape scales until the team grows enough to want sub-hubs.
2. **Channel insulation as information architecture.** `#ops` carries the routine operational chatter (research findings, inbox drafts, journal updates) without flooding the analytics signal. `#signals` carries the metrics surface without forcing every worker to read every dashboard. Hub sits on both; nobody else has to.
3. **HITL on every external surface.** Customer replies, public posts, partnership commitments — all gate. The team handles the routine 90%; the founder spends their attention on the 10% only they can do.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on.
- [Customer support](../customer-support/). The two-agent version of just the inbox slice.
- [Personal finance](../personal-finance/). Hub-and-spoke around personal money instead of product operations.
