# Example: job-finder

A three-agent team that runs your job search. The **lead** holds your applications domain and talks to you. The **scout** watches job boards on your declared criteria. The **matcher** does the deep CV-to-posting alignment and drafts cover letters.

```
lead (Claude Opus)              ← Telegram: lead bot
   owns: applications, operator-facing decisions
       │
   ┌───┴────┐
   │        │
scout    matcher
(Sonnet) (Opus)
 owns:    owns: CV/resume, fit scoring,
 source   cover letter drafts
 list,
 filter
```

You give the lead your criteria once. *"Senior platform engineering. Remote. Salary listed. Comfortable with Rust or Go. Not interested in fintech."* Scout starts watching. Matcher holds your CV. When something interesting surfaces, lead DMs you with the honest match and reasoning. You decide whether to apply.

## Why three agents earns more than one

The team passes both gates with all triggers active:

- **Entry conditions.** Lead owns applications and outcomes (state compounds: which companies ghosted, which gave fast interviews, which patterns to look for). Scout owns source quality and what's-been-seen memory. Matcher owns the canonical CV plus the calibration between postings and the operator's actual fit.
- **Work-shape triggers.** *Domain separation*: searching, scoring, and operator-facing decisions are different surfaces with different state. *Focus separation*: scout needs continuous attention to feeds; matcher is fired-off-per-posting but with deep work each time.
- **Team-shape triggers.** *Synergy*: matcher's fit-pattern memory shapes what scout knows to surface; scout's pattern of source quality shapes what matcher trusts as context. Each agent's accrued context informs the others. (Scout and matcher aren't *multiple opinions* in the methodology sense; they filter different artifacts in sequence. Synergy is the trigger doing the real work.)

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for lead).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/job-finder ~/job-search
cd ~/job-search

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace. Drop your CV/resume into ./workspace/ so matcher can read it.
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

Or use `teamctl bot setup` for the guided Telegram wizard.

## What you do with them

First message to the lead bot: tell them about you and what you're looking for.

> *"Senior platform engineer, 8 years experience. Remote-first, US/EU timezones OK. Strong in distributed systems and observability. Comfortable with Rust, Go, Python. Not interested in fintech or crypto. Salary band 200k+ USD. My resume is in workspace/resume.pdf."*

Lead briefs scout and matcher. Scout starts pulling from job boards on those criteria; matcher reads your resume and builds the canonical fit model. Within a day, you start getting surfacings.

[Screenshot of a Telegram conversation: lead surfaces a posting with honest fit (7.5/10) and reasoning, asks if you want a cover letter draft]

[Screenshot of a cover letter draft: lead surfaces the matcher's draft with the operator-facing approval prompt]

When you say *"draft me a cover letter for that one"*, matcher drafts in your voice (anchored on past drafts you've approved). Lead summarises the draft for your review. You tap ✅ to approve and save, then you send it yourself (the *send* itself is yours, since `external_email` is HITL).

## What this teaches

Three patterns layer:

1. **Domain split survives a small team.** Even with only three agents, the search/match/decide cut holds. Collapsing scout into matcher (one "find and score" agent) would lose the focus separation: scout's continuous-attention domain is different from matcher's deep-per-posting domain.
2. **Honest match scoring is the value.** A function-cut "recommender agent" might rank for engagement (show you more!). A domain-cut matcher whose state is *the calibration between you and postings* has no incentive to inflate. The structure shapes the behavior.
3. **HITL on external_email.** No applications go out without your tap. Cover letters are drafted, reviewed, and saved by the team; the actual send is yours.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on.
- [Personal research](../personal-research/). Two-agent personal-information team with similar surface-to-decide flow.
- [Market analysis](../market-analysis/). Larger team with dissent as the load-bearing trigger.
