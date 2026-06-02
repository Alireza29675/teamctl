# Example: job-finder

An agentic job-search setup that runs your search while you get on with your life. Three agents, each carrying a real per-agent stack: the **scout** sweeps job boards on your criteria, the **matcher** scores fit against your CV and drafts cover letters, and the **lead** holds the application picture and talks to you on Telegram. Nothing goes out without your tap.

```
lead · Claude Opus  ── Telegram (lead bot)
  owns applications + the operator surface
  stack: digest-drafter sub-agent
         │
   ┌─────┴──────┐
 scout         matcher
 Sonnet        Opus
 sourcing      fit scoring + drafting
 stack:        stack:
  board-        cv-fit-scorer +
  sweeper +     cover-letter-drafter
  jobs MCP      sub-agents
  (commented)
```

## What each agent carries

This is the point of the setup: every agent has a real stack, not just a prompt.

- **scout (Sonnet)** — owns sourcing. Its **board-sweeper** sub-agent does the per-cycle pull across your source list and public career pages — over the web, no API key, working out of the box — dedupes against what's been seen, and returns a tight batch. For a live jobs-API feed, scout also takes an optional **job-board MCP**: there's no official published one yet, so it ships commented out in `projects/jobs.yaml` with a pointer to a free community server (Adzuna) you can wire in.
- **matcher (Opus)** — owns the match. Its **cv-fit-scorer** sub-agent reads a posting against your CV and returns an honest fit number *with reasoning*; its **cover-letter-drafter** writes in your voice, anchored on the real fit points, when you ask. Calibrated, not flattering — a 4/10 comes back as a 4/10.
- **lead (Opus)** — owns the applications and the Telegram conversation. Its **digest-drafter** sub-agent turns the application tracker and the week's activity into a short "state of the search" read. The lead decides what reaches you, and when.

## The loop, and your surface

You give the lead your criteria once. *"Senior platform engineering. Remote. Salary listed. Comfortable with Rust or Go. Not interested in fintech."* Scout starts sweeping; matcher reads your CV and builds the fit model. When something interesting surfaces, the lead DMs you the honest match and the reasoning behind it. You decide whether to apply — and the *send* is always yours, since `external_email` is HITL.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for the lead).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/job-finder ~/job-search
cd ~/job-search

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace. Drop your CV/resume into ./workspace/ so the matcher can read it.
mkdir -p workspace
# cp ~/resume.pdf workspace/
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

`teamctl up` brings the team up in tmux and starts the lead's Telegram bot for you — one bot per Telegram manager, no separate process to launch. `teamctl status` shows the agents; `teamctl bot status` shows the bot. New to the Telegram side? `teamctl bot setup` is a guided wizard that fills the token + chat id in for you.

## What you do with them

First message to the lead bot: tell it about you and what you're looking for.

> *"Senior platform engineer, 8 years experience. Remote-first, US/EU timezones OK. Strong in distributed systems and observability. Comfortable with Rust, Go, Python. Not interested in fintech or crypto. Salary band 200k+ USD. My resume is in workspace/resume.pdf."*

The lead briefs scout and matcher. Scout starts pulling from job boards on those criteria; matcher reads your resume and builds the canonical fit model. Within a day, the surfacings start: the lead DMs you a posting with an honest fit (say 7.5/10) and the reasoning, and asks whether you want a cover letter.

When you say *"draft me a cover letter for that one"*, the matcher drafts in your voice — anchored on past drafts you've approved — and the lead summarises it for your review. You approve and save; then you send it yourself, since the *send* is gated behind `external_email`.

## What this demonstrates

- **Per-agent stacks are the setup.** Each agent carries the sub-agents (and, for scout, the MCP) its job actually needs — sourcing, scoring, drafting, and the weekly digest are real capabilities wired onto the agents that own them, not one do-everything prompt.
- **A real MCP, honestly wired.** scout's optional jobs feed shows what a per-agent `mcps:` block looks like — and, because no official job-board MCP exists yet, how to wire a community one without dressing a fabricated package up as real. Its board-sweeper sub-agent keeps scout useful from the first `teamctl up`, with or without the MCP.
- **HITL where it counts.** No application goes out without your tap. Cover letters are drafted, reviewed, and saved by the team; the actual send is yours, because `external_email` is on the globally-sensitive list.

## Teardown

```bash
teamctl down
rm -rf .team/state/
```

## Related

- [team-compose.yaml reference](https://teamctl.run/reference/team-compose-yaml/) — the `subagents:` and `mcps:` fields this example leans on.
- [personal-finance](../personal-finance/) — a three-agent setup that holds the picture of your money: live tracking, anomaly flags, and read-only digests on Telegram.
- [How to think about agent teams](https://teamctl.run/concepts/teams/) — the methodology behind team design.
