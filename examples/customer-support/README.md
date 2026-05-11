# Example: customer-support

A two-agent team that runs your support inbox. **Triage** reads everything coming in, decides what gets the operator's eyes, what gets auto-closed, and what gets a drafted reply. **Drafter** writes the reply in your voice; you tap to send. The team is HITL on every external send.

```
triage (Claude Opus)              ← Telegram: triage bot
   owns: inbox routing, operator-attention filter
       ↕
drafter (Claude Opus)
   owns: voice-tuned customer replies
```

Triage is your inbox-first-read. Drafter is your voice. The operator sees the right 10% of tickets through Telegram (escalations + drafts for approval); the team handles the routine 90%.

## Why two agents earns more than one

The team passes both gates with synergy as the load-bearing trigger:

- **Entry conditions.** Triage owns the routing-decisions domain end-to-end (state: category model, pattern memory, the operator's calibration on what's worth their eyes). Drafter owns the voice-tuned-replies domain end-to-end (state: voice profile, learning from approvals/rewrites).
- **Work-shape triggers.** *Domain separation*: routing decisions and voice-prose drafts are different surfaces with different state. Triage's calibration sharpens from operator decisions on escalations; drafter's voice profile sharpens from operator rewrites on drafts.
- **Team-shape trigger.** *Synergy*: triage's tone notes shape drafter's prose; drafter's draft-quality feeds back into how triage decides what's draft-worthy vs auto-closeable. Each filter informs the other.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for triage).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/customer-support ~/support-team
cd ~/support-team

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace dir. Where triage caches the category model and drafter caches the voice profile.
#    The inbox itself should be wired up here too (forwarded email, support tool exports,
#    or however your support tickets arrive).
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

First message to triage: tell them how you want to handle tickets. Be specific.

> *"My voice is warm-direct. Acknowledge frustration before solving. Always include the relevant docs link. Never promise roadmap dates. Sign off as 'the team' (we're a small company). Escalate anything from Customer X or Customer Y; they pay >$10k/year. Anything mentioning 'refund' escalates; I'll handle those myself."*

The team starts watching the inbox. Examples of what you'll see on Telegram:

> *Queue check: 6 since this morning. 3 auto-closed (FAQ-shaped: how-do-I-X). 2 drafted for your approval — Customer A about a billing question, Customer B about a feature request — both coming through next. 1 escalation: Customer C's second angry message this week, copy-pasted below.*

> *Draft for Customer A (billing): acknowledged plan confusion, explained Free vs Pro, offered upgrade link. Approve? [tap]*

> *Weekly patterns: 4 tickets this week hit the same OAuth-callback edge case — looks like a docs gap. Want me to draft a docs update for your team to publish?*

## What this teaches

Three patterns layer:

1. **HITL on every external send.** Drafter never sends without the operator's tap. The team handles 90% of the routine work; the operator handles the 10% that needs them. The 10% is the value.
2. **Voice is a domain.** Drafter's voice profile compounds over time from approvals and rewrites. A drafter that doesn't learn from corrections is a generic LLM; a drafter that sharpens toward the operator's voice every week is a real teammate.
3. **The triage decision is the multiplier.** A good triage agent saves the operator dozens of unnecessary read-throughs every day. A bad one wastes attention. The right architecture isn't "more agents drafting more replies"; it's "one agent routing perceptively + one drafting in voice."

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on.
- [Personal newsletter / research](../personal-research/). Two-agent personal team with a similar synergy-shaped pattern.
- [Solo founder ops](../solo-founder-ops/). Hub-and-spoke team where customer support is one slice among many.
