# Example: personal-research

An agentic setup that owns your information life. A **curator** follows the world on the topics you care about; a **buddy** holds your compounding mental model and is the only one you talk to. Two agents, each with a real capability stack, running a daily loop so the signal finds you instead of the other way around.

```
buddy (Claude Opus)              ← Telegram: your thinking partner
   stack: summarizer + synthesizer subagents
   owns: research queue, compounding mental model, operator-facing voice
       ↕
curator (Claude Sonnet)
   stack: search/news MCP + source-scout subagent
   owns: source list, interest filter, daily news loop
```

You tell the buddy what you're curious about. The buddy briefs the curator. The curator runs a daily loop — searching its news MCP, scouting new sources, filtering for signal — and DMs the buddy what's worth a look. The buddy filters again against your voice and your timing, and brings one or two things to your phone if any earned it. Most days nothing crosses the bar. That's the right output.

## The per-agent stacks

The point of this setup is that each agent carries real capabilities, not just a prompt:

- **curator** runs a **search/news MCP** (Brave Search in the shipped config — swap in any search/news server) so its daily pull hits fresh, recency-ranked results, plus a **`source-scout`** subagent that researches *where to look* when you add a new interest: who's worth following, which feeds carry signal, what's noise.
- **buddy** runs a **`summarizer`** subagent that turns a source into the cutting paragraph (lead with what's new, not a recap) and a **`synthesizer`** subagent that connects a fresh finding to what you concluded weeks ago — what it confirms, what it contradicts, what compounds.

Subagents live in `.team/subagents/` and are claude-only; the MCP is runtime-agnostic. The wiring is in `.team/projects/research.yaml`.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather. Get your chat id from @userinfobot.

# 3. A Brave Search API key for the curator's news MCP (free tier at
#    https://brave.com/search/api/). Swap the `search` MCP in
#    projects/research.yaml if you prefer a different search/news server.

# 4. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/personal-research ~/research-buddy
cd ~/research-buddy

# 5. Fill in the Telegram token + chat id + Brave key.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 6. Workspace dir. Where the buddy keeps notes and the curator caches sources.
mkdir -p workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up      # also starts the buddy's Telegram bot
teamctl status
```

`teamctl up` already starts the buddy's Telegram bot — one poller per manager with a `telegram:` block. Watch for the `up · bot … → research:buddy` line to confirm it came up. Don't launch `team-bot` yourself: a second poller on the same token triggers a Telegram **409 conflict**. (A `skip · bot … unset` line instead means the token isn't in `.env` yet — fill it and rerun.) Prefer a guided setup? `teamctl bot setup` walks the token wiring.

## What you do with them

DM the buddy bot about anything you're curious about. A half-remembered concept, an article you want chewed over, a question you've been circling.

- *"I keep hearing about ZK rollups; explain them like I already understand cryptography but not blockchain?"*
- *"Read this paper and tell me what's actually new vs. previously known."*
- *"I asked you about CAP theorem two weeks ago. Has my thinking on it held up?"*

Tell the buddy what you want followed. *"I care about AI agent orchestration, post-training research, and what's happening with TeamOps as a category."* The buddy briefs the curator. The curator starts watching the world. Once a day (or whenever something breaks), the curator surfaces what matters. The buddy filters again and brings 1-2 items to your phone if any earned it. Most days nothing crosses the bar. That's the right output.

[Screenshot of a Telegram conversation: user asks "remind me what we settled on for X" and the buddy responds with a connected answer pulling from prior research]

[Screenshot of a proactive surfacing: buddy DMs *"Curator surfaced something on TeamOps you might want to see. Short version: ..."*]

## What this setup shows

Three patterns layer:

1. **Per-agent stacks, not just prompts.** Each agent carries real capabilities sized to its job: the curator gets a search/news MCP and a source-scout because its work is *finding*; the buddy gets summarizer/synthesizer subagents because its work is *framing*. Capabilities follow the domain, not the org chart.
2. **Filters compose.** Curator filters "what's worth surfacing" against the operator's interests. Buddy filters "what's worth saying" against the operator's voice and timing. Two filters land less noise than one, and each improves over time from the other's signal.
3. **HITL on outbound.** Researching and surfacing are free. Sending an email to a researcher you don't know yet is not. `external_email` is on the globally-sensitive list, so both agents pause for your tap before reaching out on your behalf.

## Teardown

```bash
teamctl down
rm -rf .team/state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology behind the team shape.
- [Market analysis](../market-analysis/). A medium team where dissent is the load-bearing trigger.
- [OSS maintainer](../oss-maintainer/). A pipeline team with per-agent stacks and plan-mode HITL on release.
