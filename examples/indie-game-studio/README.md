# Example: indie-game-studio

A solo indie game dev's brain trust — a **director** who steers the
vision, a **designer** working on systems and mechanics, a **writer**
on narrative and dialogue, and a read-only **playtest critic** who
quietly tells the director when an idea won't survive contact with a
real player. You, the dev, talk to the director through one Telegram
bot.

```
director (Claude Opus)              ← Telegram: director bot
  ├─ designer        (Claude Opus)
  ├─ writer          (Claude Sonnet)
  └─ playtest_critic (Claude Opus, plan-mode · read-only)
                       ↳ private back-channel to director only
```

The critic lives on a separate `#critique` channel that only the
director can see. The designer and writer never hear the critique
directly — that's the point. Half-finished ideas need room to breathe
before they get stress-tested.

## Install

```bash
# 1. Install teamctl + the Claude Code runtime.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather.
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/indie-game-studio ~/studio
cd ~/studio

# 4. Fill in token + chat id.
cp .env.example .env
$EDITOR .env

# 5. Workspace dir (where design docs and prototype notes will live).
mkdir -p workspace
```

## Run

```bash
set -a; . ./.env; set +a

teamctl validate
teamctl up
teamctl status
```

Now start the director's Telegram bot:

```bash
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$DIRECTOR_BOT_TOKEN" \
  --authorized-chat-ids "$DIRECTOR_CHAT_IDS" \
  --manager studio:director
```

DM the director with the thing you're stuck on — a mechanic that feels
flat, a boss that isn't landing, a story beat that needs sharpening.

## What this demonstrates

A **plan-mode dissenter on a creative pipeline**, with a **private
critique channel**. `playtest_critic` runs in `permission_mode: plan`
— it can read every design doc, but it cannot mutate any of them. Its
only output is critique (and counter-proposals) routed privately to
the director. The pattern is the same shape as `market-analysts`'
`quant_risk`, retargeted at creative judgement instead of financial
risk.

The trick is the channel split: `#design` is the open workshop where
designer and writer iterate; `#critique` is the back room where the
director and the critic talk frankly without disrupting that
iteration. The director chooses what, if anything, to bring back into
`#design`.

## Shape of a typical session

1. Dev DMs director: *"the boss fight in act 2 isn't landing in
   playtests."*
2. Director posts a one-paragraph framing to `#design`. Designer
   proposes a tweak (say, telegraph the wind-up earlier). Writer
   suggests a beat of dialogue that lampshades the tell.
3. Director DMs the synthesis to `playtest_critic`. Critic reads the
   design history, posts a pre-mortem to `#critique`: *"this works if
   players have already learned wind-up reads in act 1; if they
   haven't, you've front-loaded the difficulty curve. What if the
   tutorial boss in act 1 used a 200ms longer wind-up so the read is
   pre-trained?"*
4. Director picks: ship the change, fold in the critic's
   counter-proposal, or tell the critic *"noted, we're keeping it"*.
5. Director reports back to the dev with the chosen path and the
   reasoning.

## Teardown

```bash
teamctl down
rm -rf state/
```
