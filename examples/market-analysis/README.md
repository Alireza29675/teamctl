# Example: market-analysis

One possible TeamCTL team definition: a four-agent research desk with separate roles for data, interpretation, dissent and synthesis. Their channels support discussion before the chief brings findings to Telegram.

The roles direct agents to observe rather than trade. Review data access, sources and runtime permissions before running it; the setup does not guarantee correct analysis or enforce read-only access through a policy label alone.

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

The role instructions exclude trading and moving money. The `trade`, `payment` and `external_email` policy labels do not intercept arbitrary tool calls. Use appropriate tool credentials and runtime permissions, and independently check any findings.

## Why this definition separates four roles

This structure separates ongoing responsibilities and independent perspectives:

- **Entry conditions**; each agent owns a real domain end-to-end. Data feeds, narrative read, dissent, synthesis. All four have rhythms, compounding state, and reasons to keep going.
- **Work-shape triggers**; *domain separation* and *focus separation* both fire. The data domain is a different surface from the read; the read is a different surface from the dissent; the synthesis is a different surface still.
- **Separate perspectives.** Interpreter and risk are instructed to challenge each other's reasoning; chief synthesizes their evidence and disagreements. Check whether that discussion adds value before keeping four sessions running.

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

# 5. Workspace; where the desk caches data, transcripts, and notes.
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

- **Proactively**; rare. When the desk converges on something real (collector flags a notable print, interpreter lands a thesis, risk signs off or escalates), chief DMs you with the compact synthesis. Signal over noise; most days are quiet.
- **On demand**; DM the chief bot with anything: *"what's the read on 2y yields?"*, *"is the EUR move pre-FOMC overdone?"*, *"why did equities reverse yesterday?"*. Chief routes internally and circles back with the integrated answer.
- **Scheduled**; a daily close note to `#alerts` summarising what moved, what didn't, and the desk's working theses.

Every message you get ends with **"Not advice; observation only."** The desk reads the market; you decide what to do with the read.

## What this teaches

Three patterns layer:

1. **Dissent as a domain.** Risk runs in `permission_mode: plan` and exists to disagree. The desk is stronger because someone is paid to push back on the working thesis. Try collapsing risk into interpreter and you lose the *multiple opinions* trigger that makes the team valuable.
2. **Channel insulation by purpose.** `#desk` is where the analysts argue; `#alerts` is where the operator-shaped output goes. Collector posts data prints to `#desk` only; that conversation doesn't need to flood `#alerts`. Channel design is the team's information architecture.
3. **An observational scope.** The roles exclude financial actions and identify decisions that need your input. Runtime and tool permissions provide the actual action boundary.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [Designing an agent team](https://teamctl.run/concepts/teams/); peer perspectives, responsibility boundaries and other possible team shapes.
- [Newsletter office](../newsletter-office/); separate newsroom and website rosters connected through a bridge.
- [Product team](../product-team/); product and engineering roles with cross-model review.
