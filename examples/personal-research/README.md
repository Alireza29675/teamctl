# Example: personal-research

A two-agent team that owns your information life. The **buddy** holds your reading list, your compounding mental models, and the questions you've been chewing on. The **curator** runs on a loop, following the world on the topics you care about, and surfaces what matters. You talk to the buddy on Telegram. The buddy talks to you.

```
buddy (Claude Opus)              ← Telegram: buddy bot
   owns: research queue, compounding mental model, operator-facing voice
       ↕
curator (Claude Sonnet)
   owns: source list, interest filter, daily news loop
```

The buddy is your thinking partner. Questions you ask, papers you point at, half-formed thoughts you want chewed over. The curator is your information scout. Every cycle, they pull from sources you (and the buddy) have tuned, filter for signal, and surface the worthwhile to the buddy. The buddy decides what (if anything) to bring to your phone.

## Why two agents earns more than one

The team passes both gates with two triggers active:

- **Entry conditions.** Buddy owns the research/mental-model domain end-to-end. Curator owns the sourcing/filter domain end-to-end. Both accumulate compounding state (buddy: your mental model, decisions made; curator: source quality memory, filter calibration).
- **Work-shape triggers.** *Domain separation* fires for both: research-on-demand and proactive-news-following are different surfaces with different state. *Focus separation* fires for curator (continuous attention to feeds, not fired-off per question).
- **Team-shape triggers.** *Multiple opinions*: curator's "what's worth surfacing" is one filter; buddy's "what's worth bringing to the operator" is a second filter. Two filters land less noise than one. *Synergy*: the buddy's mental model of the operator's interests informs the curator's filter over time, and the curator's surfacings feed back into the buddy's model. Compounding both ways.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather. Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/personal-research ~/research-buddy
cd ~/research-buddy

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir. Where the buddy keeps notes and the curator caches sources.
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

DM the buddy bot about anything you're curious about. A half-remembered concept, an article you want chewed over, a question you've been circling.

- *"I keep hearing about ZK rollups; explain them like I already understand cryptography but not blockchain?"*
- *"Read this paper and tell me what's actually new vs. previously known."*
- *"I asked you about CAP theorem two weeks ago. Has my thinking on it held up?"*

Tell the buddy what you want followed. *"I care about AI agent orchestration, post-training research, and what's happening with TeamOps as a category."* The buddy briefs the curator. The curator starts watching the world. Once a day (or whenever something breaks), the curator surfaces what matters. The buddy filters again and brings 1-2 items to your phone if any earned it. Most days nothing crosses the bar. That's the right output.

[Screenshot of a Telegram conversation: user asks "remind me what we settled on for X" and the buddy responds with a connected answer pulling from prior research]

[Screenshot of a proactive surfacing: buddy DMs *"Curator surfaced something on TeamOps you might want to see. Short version: ..."*]

## What this teaches

Three patterns layer:

1. **Domains, not functions, even at small scale.** Two agents in this team. Neither is "the researcher" or "the writer." They own *things*. Buddy owns the operator-facing relationship and the compounding mental model. Curator owns the sourcing/filter loop. The work passes between them; the state stays where it belongs.
2. **Filters compose.** Curator filters "what's worth surfacing" against the operator's interests. Buddy filters "what's worth saying" against the operator's voice and timing. Two filters land less noise than one, and each filter improves over time from the other's signal.
3. **HITL on outbound.** Researching and surfacing are free. Sending an email to a researcher you don't know yet is not. `external_email` is on the globally-sensitive list. Both agents ask before reaching out on your behalf.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on.
- [Market analysis](../market-analysis/). Medium team where dissent is the load-bearing trigger.
- [SaaS product team](../saas-product/). Large team where domain-cut survives at scale.
