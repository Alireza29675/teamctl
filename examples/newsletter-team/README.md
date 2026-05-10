# Example: newsletter-team

A two-agent newsletter that lands in your Telegram every morning, tuned to the topics you actually care about. An **editor** owns the voice and the publish call; a **curator** owns the source list and the filter. They push against each other; you get a sharper newsletter than either could write alone.

```
editor (Claude Opus)              ← Telegram: editor bot
   owns: voice, framing, publish decisions
   ↕
curator (Claude Sonnet)
   owns: source list, filter criteria, source memory
```

Editor talks to you on Telegram. Curator works upstream — you don't DM them directly; they refine the candidate pool that editor draws from. Together they make a team because they disagree about what to publish.

## Why two agents earns more than one

The newsletter passes both gates with the team-shape trigger firing — that's what makes it a real team rather than a single agent doing both jobs:

- **Entry conditions** — editor owns the voice/publish domain end-to-end, curator owns the sourcing/filter domain end-to-end. Both have rhythms (daily cycle), both accrue context (voice tuning, source quality memory).
- **Work-shape trigger** — *domain separation*: voice and sourcing are different surfaces with different state. Combining them into one agent collapses the editorial discipline of two filters into one.
- **Team-shape trigger** — *multiple opinions*: editor and curator have different perspectives. Curator surfaces what's interesting; editor surfaces what fits the voice. Both filters apply. A solo agent doing both would always agree with itself.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for editor).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/newsletter-team ~/newsletter
cd ~/newsletter

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace — where the team caches sources, voice notes, and what's been published.
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

## Tuning the newsletter to your taste

DM the editor bot with what kind of newsletter you want. Be specific:

- *"Daily AI news, tuned to what's actually new and not just press releases. Skip launches; prefer essays and papers."*
- *"Markets digest, US session focus, no crypto, ~3 stories at most."*
- *"Indie game dev signal — release announcements, postmortems, novel mechanics. Skip funding news."*

Editor briefs curator; curator tunes the filter. Over the next few cycles you'll see the candidate batches sharpen toward your taste. When you don't like a story that landed, tell editor — they'll log the reaction and the voice drifts accordingly.

[Screenshot of a Telegram morning newsletter: 3 stories, each with a one-line "why this matters" framing and the source link]

[Screenshot of the user tweaking taste: "skip launches please" and the editor confirming "got it — filtering them out starting tomorrow"]

## Publishing flow

Every send goes through HITL. The bundle arrives in your Telegram as an approval prompt — 3-5 stories with their framings. You tap ✅ and it goes out (to wherever you're sending it — email subscribers, your blog, your own Telegram channel; the *destination* is your choice, the team's job ends at the approved bundle).

If a story doesn't pass your sniff test, tap ✗ and tell editor why. The reason becomes voice feedback.

## What this teaches

Three patterns layer:

1. **Two domain-shaped agents > two function-shaped agents.** Curator and editor aren't "the researcher" and "the writer" — they own different *things*. The filter is curator's domain; the voice is editor's domain. Even though their work overlaps in time, the state each holds is distinct.
2. **Multi-opinions trigger in action.** The newsletter is the *back-and-forth* between two agents who disagree about what's worth publishing. That disagreement is the team's value; collapsing it removes the point.
3. **HITL on publish keeps editorial trust honest.** A newsletter that ships without your tap is one mistake away from sending the wrong thing under your name. The approval moment is feature, not friction.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/) — the methodology this example is built on.
- [Personal research](../personal-research/) — single-agent team for when you don't need the two-filter shape.
- [Market analysis](../market-analysis/) — medium team where domain-cut and multi-opinions both fire harder.
