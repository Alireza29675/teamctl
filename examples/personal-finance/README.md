# Example: personal-finance

An agentic setup that holds the picture of your money so you don't have to — built around a real financial-data feed. Three agents, each with its own stack: the **tracker** pulls live account data through Plaid's official MCP and flags anomalies, the **analyst** builds the long-arc patterns, and **books** decides what's worth your attention and tells you on Telegram. Read-only by design: nothing moves money without your tap.

```
books · Claude Opus  ── Telegram (books bot)
  owns the operator surface + the "what matters" filter
  stack: briefing-drafter sub-agent
         │
   ┌─────┴──────┐
 tracker       analyst
 Sonnet        Opus
 live data     synthesis
 stack:        stack:
  Plaid MCP +   trend-analyzer +
  anomaly-      digest-writer
  scanner       sub-agents
  sub-agent
```

## What each agent carries

This is the point of the setup: every agent has a real stack, not just a prompt.

- **tracker (Sonnet)** — owns the live data. Its headline is a real **financial-data MCP**: Plaid's official server (`mcp-server-plaid`), wired straight onto the agent. Out of the box it runs against Plaid's free **sandbox** — mock accounts and transactions, the honest way to see the whole loop work end to end — and points at your production Plaid setup for real data. Its **anomaly-scanner** sub-agent flags transactions that break *your own* baseline, with the evidence attached.
- **analyst (Opus)** — owns the long arc. Its **trend-analyzer** sub-agent does the longitudinal math (savings rate, category drift, holdings), and its **digest-writer** turns those trends into the structured weekly/monthly digest it hands books. Patterns earn their place; a boring month reads as "nothing material changed."
- **books (Opus)** — owns the operator surface. Its **briefing-drafter** sub-agent turns whatever tracker or analyst surfaced into a short, calm Telegram message in your voice — but only once books has decided it earns a ping. Most days, nothing lands. That's the right output.

## The loop, and your surface

You give books your priorities once. *"I care about my savings rate, my emergency-fund weeks-of-runway, and not silently overspending on subscriptions."* The team starts watching: tracker refreshes and scans, analyst synthesises, books filters everything against what you actually care about. When something matters — an anomaly, a developing pattern, a milestone — books DMs you the specific facts and offers depth. The team is **read-only**: `payment` is HITL-gated, so nothing moves money, and even proposing a specific trade is out of scope. You get the picture; you decide the action.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for books).
#    Get your chat id from @userinfobot.

# 3. tracker's MCP is Plaid's official server, mcp-server-plaid — the
#    PyPI package (run via uvx/pip), NOT the unrelated npm package of the
#    same name. Install uv for uvx (https://docs.astral.sh/uv/), or
#    `pip install mcp-server-plaid` and switch the tracker's mcps block to
#    command: python, args: [-m, mcp_server_plaid].

# 4. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/personal-finance ~/finance-team
cd ~/finance-team

# 5. Fill in token + chat id. For the live feed, paste free Plaid Sandbox
#    keys (dashboard.plaid.com) into PLAID_CLIENT_ID / PLAID_SECRET. Leave
#    them blank and the team still comes up — tracker just has no feed yet.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 6. Workspace dir. Where the tracker caches account data and the
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

`teamctl up` brings the team up in tmux and starts the books Telegram bot for you — one bot per Telegram manager, no separate process to launch. `teamctl status` shows the agents; `teamctl bot status` shows the bot. New to the Telegram side? `teamctl bot setup` is a guided wizard that fills the token + chat id in for you.

## What you do with them

Tell books what you care about. *"My priorities: savings rate above 20%, emergency fund at 6 months expenses, no surprise subscriptions, alert me on transactions over $300."*

The team starts watching. Anomalies surface from tracker; patterns develop in analyst; books decides what earns Telegram. Examples of what books might send you:

> *Anomaly worth a look: $487 at Hardware Store Z on Tuesday. Your average transaction in that account is $40; this is 12x. No previous transactions there in 6 months. Was this you?*

> *October digest: savings rate held at 22%, just below your 20% floor. Dining-out spent 40% above last month, mostly from one week. Holdings: tech allocation now 47%, drifting from your 40% target. Want a deeper look at any of these?*

> *Subscriptions watch: three new recurring charges added this quarter (Service A $12/mo, Service B $9/mo, Service C $24/mo). Cumulative monthly subscription spend is up $45 since July.*

## What this demonstrates

- **A real financial-data MCP, wired onto the agent that owns the data.** tracker's `mcps:` block is Plaid's official server, runnable today against the free sandbox — a concrete example of a per-agent MCP, shipped honest about being sandbox-by-default rather than pretending a fabricated server reads your real bank.
- **The signal/noise problem is a stack of filters.** The anomaly-scanner filters transactions against your baseline; the trend-analyzer separates patterns from outliers; books filters everything against *what's worth a Telegram*. Three filters land far less noise than one, and each lives on the agent that owns that judgment.
- **Read-only by design.** `payment` is on the globally-sensitive list, so the team can see your money but never move it. You get an informed read — built from a real feed and compounding history — and you make every call.

## Teardown

```bash
teamctl down
rm -rf .team/state/
```

## Related

- [team-compose.yaml reference](https://teamctl.run/reference/team-compose-yaml/) — the `mcps:` and `subagents:` fields this example leans on.
- [job-finder](../job-finder/) — a three-agent setup that runs your job search: a scout on the boards, a matcher on your CV, an operator-facing lead.
- [personal-research](../personal-research/) — a two-agent personal-information loop: a reading buddy that holds your interests and a curator that follows the news.
- [How to think about agent teams](https://teamctl.run/concepts/teams/) — the methodology behind team design.
