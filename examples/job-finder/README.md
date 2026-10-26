# Example: job-finder

One possible TeamCTL team definition for a job search: a **scout** finds postings, a **matcher** compares them with your CV, and a **lead** tracks applications and talks to you on Telegram. You choose the criteria and review drafts.

Adapt the roles and tools to your search. The instructions leave sending applications to you; configure runtime permissions to match that boundary. Background work needs an awake host and active, authenticated sessions.

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

Give the lead your search criteria. Scout's role covers finding postings; matcher compares them with your CV. The lead brings the fit reasoning for your review. The instructions leave sending applications to you; the sensitive-action list records that policy rather than intercepting tools.

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

The intended workflow is for the lead to brief scout and matcher, compare sourced postings with your resume, and bring you the fit reasoning before drafting a cover letter. Check the evidence behind any score.

When you say *"draft me a cover letter for that one"*, the matcher drafts and the lead summarizes it for review. You check, save and send the application yourself.

## What this demonstrates

- **Per-agent stacks are the setup.** Each agent carries the sub-agents (and, for scout, the MCP) its job actually needs — sourcing, scoring, drafting, and the weekly digest are real capabilities wired onto the agents that own them, not one do-everything prompt.
- **A real MCP, honestly wired.** scout's optional jobs feed shows what a per-agent `mcps:` block looks like — and, because no official job-board MCP exists yet, how to wire a community one without dressing a fabricated package up as real. Its board-sweeper sub-agent keeps scout useful from the first `teamctl up`, with or without the MCP.
- **A clear sending boundary.** The role instructions ask the team to draft, review and save cover letters while leaving submission to you. Runtime permissions and connected tools should match that scope.

## Teardown

```bash
teamctl down
rm -rf .team/state/
```

## Related

- [team-compose.yaml reference](https://teamctl.run/reference/team-compose-yaml/) — the `subagents:` and `mcps:` fields this example leans on.
- [personal-finance](../personal-finance/) — a three-agent setup that holds the picture of your money: live tracking, anomaly flags, and read-only digests on Telegram.
- [Designing an agent team](https://teamctl.run/concepts/teams/) — the methodology behind team design.
