# Example: personal-research

A single-agent team — a research buddy that owns your reading list, your half-finished mental models, and the compounding knowledge of what you actually care about. You reach them through one Telegram bot. They remember.

```
buddy (Claude Opus)              ← Telegram: buddy bot
   owns: your research queue, your compounding mental model
```

That's the whole team. One agent.

## Why one agent earns its place

Single-agent teamctl teams are valid. The buddy passes both gates:

- **Entry conditions** — they own a real domain end-to-end (your research queue), they have time-management (the queue rhythm is theirs), and persistent memory is the whole point (a research agent without memory is a worse search engine).
- **Situational trigger** — *domain separation*: your research queue has state, history, and decisions that compound. A new question this week is informed by a question you asked last month.

The team-shape triggers (multiple opinions, synergy) don't apply here — those need 2+ agents. That's fine. Persistence earned through ownership alone is a real shape.

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

# 5. Workspace dir — where the buddy keeps notes, summaries, and source caches.
mkdir -p workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Then start the buddy's Telegram bot — or use `teamctl bot setup` for the guided wizard.

## What you do with them

DM the buddy bot anything you're curious about — a half-remembered concept, an article you want chewed over, a question you've been circling.

- *"I keep hearing about ZK rollups; can you explain them like I already understand cryptography but not blockchain?"*
- *"Read this paper and tell me what's actually new vs. previously known."* (paste link)
- *"I asked you about CAP theorem two weeks ago — has my thinking on it held up?"*

They'll answer the one question you asked. If they need to research, they'll say so and come back. If they noticed a contradiction between two things you've explored, they'll surface it on their own.

[Screenshot of a Telegram conversation: user asks "remind me what we settled on for X" and the buddy responds with a connected answer pulling from prior research]

## What this teaches

This is the smallest valid teamctl team and a real one. Three patterns layer:

1. **Persistence earned by ownership alone.** No workers, no `reports_to`, no channels. The buddy passes both gates by being a real domain owner with compounding memory. If your work has a single domain that needs persistence, you don't need a multi-agent team.
2. **The harness can't replace memory.** Sub-agents (the Task tool in Claude Code) have isolated contexts. A buddy that remembers your research two months later isn't doable as a sub-agent — that's why it's a teamctl agent.
3. **HITL on outbound.** Researching is free. Sending an email to a researcher you don't know yet is not. `external_email` is on the globally-sensitive list; the buddy asks before reaching out to the world on your behalf.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/) — the methodology this example is built on, including the two-gate framing.
- [Newsletter team](../newsletter-team/) — a small 2-agent team where the team-shape trigger *multiple opinions* fires.
