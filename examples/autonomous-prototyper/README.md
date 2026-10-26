# Example: autonomous-prototyper

One possible TeamCTL team definition for researching ideas and building prototypes. An **ideator** develops proposals, a **pessimist** challenges their evidence, and a **Codex prototyper** builds an approved experiment.

You define the direction and review the proposals on Telegram. A host scheduler sends the messages that start each research cycle; active sessions, an awake host and provider access are required. The roles describe the intended behavior and do not guarantee a useful idea or working product.

```
  OS cron ──"run a cycle"──▶  ┌─ ideator  (Claude Opus)  ← Telegram: settle the direction, then approve / reject ideas
  (the heartbeat)             │
                              ├─ pessimist  (Claude · plan-mode)  · tries to KILL every idea — only survivors reach you
                              └─ prototyper (Codex)               · builds a throwaway proof of an approved idea

channels:   #ideation  ideator ↔ pessimist (the kill room)      #build  ideator ↔ prototyper (approved-spec handoff)
```

The team works in the bundled [`workspace/`](workspace/): it reads the
shipped [`workspace/seed.md`](workspace/seed.md) (rough starting ideas),
writes the settled `direction.md` there, queues vetted specs under
`ideas/`, and builds throwaways under `prototypes/`. Point it at your own
interests by editing one file (see [Point it at your own
hunt](#point-it-at-your-own-hunt)).

## The two-phase loop

The whole design turns on a single gate: **`direction.md`**. Until it
exists, the hunt doesn't run. You write it *with* the ideator, once.

### Phase (i) — settle the direction (interactive, no clock)

1. **You → ideator** (Telegram): talk through what you're chasing. The
   ideator opens from `seed.md`, does light research to ground the
   conversation, and converges with you on a direction.
2. When you settle, the ideator writes **`direction.md`** — the hunt
   charter — and tells you it's going autonomous. **This is the gate.**

### Phase (ii) — background research (cron-driven)

3. **ideator** (on each cron poke): researches its `direction.md` domain
   for a real gap → drafts one startup idea → posts it to the pessimist on
   `#ideation`.
4. **pessimist**: throws its kill-stack at it — *already exists?*
   (`prior-art-checker`), *anyone want it?* (`product-researcher`),
   *buildable cheaply?* (`feasibility-analyst`). Returns a verdict:
   **killed** (with the fatal reason) or **survived** (with what it
   couldn't refute). Its default is killed.
5. **ideator**: drops the dead; for a survivor, presents it to you with
   `request_approval` — *with the pessimist's verdict attached*, so you
   see what was already tried against it.
6. **You**: approve or reject. (This is the product loop, not a safety
   rail — it's the whole point of the human surface.)
7. On approval, the **ideator** specs the idea and hands it to the
   **prototyper** on `#build`.
8. **prototyper**: builds a throwaway under `prototypes/<id>/`, reports
   what worked and what's faked. Anything that would reach outside
   (`publish`/`deploy`) stops for your approval.

You can adjust the direction, inspect the research and decide which experiments to build.

## What this demonstrates

Three configuration patterns:

1. **The adversarial gate is real research, not vibes.** The pessimist
   doesn't just *say* "this won't work" — it runs `prior-art-checker`,
   `feasibility-analyst`, and `product-researcher` to *try to kill the
   idea*, and a kill has to cite something real (a free incumbent, a
   broken cost model, an imaginary wedge). Only ideas that survive a
   genuine kill-attempt reach you. The value is in what it **rejects**.
2. **Two phases, and the human's role inverts between them.** In Phase
   (i) you *lead* — you and the ideator settle the direction together. In
   Phase (ii) the machine leads — it proposes, you only approve or reject.
   You set the *what* once, up front, then step back to curator.
3. **Autonomy with an honest clock.** Once the direction is settled, the
   ideator runs a generate → vet → queue loop on a cadence, so you walk up
   to a curated shortlist instead of a blank prompt. The clock that drives
   it is an **OS scheduler**, not a timer inside TeamCTL — see below.

## Schedule the research cycles

TeamCTL delivers messages to agent sessions. This example uses an external scheduler to send the messages that trigger research cycles; TeamCTL does not provide that clock.

The heartbeat is a host **cron** (Linux/macOS) or **launchd** (macOS) job
that runs, on a cadence:

```bash
teamctl send autonomous-prototyper:ideator "run a cycle"
```

`teamctl send` inserts an ordinary mailbox message; the team's MCP channel
watcher sees it and wakes the ideator exactly as a teammate's message
would. No TeamCTL process runs "on a schedule" — only the team
(`teamctl up`) and the cron entry. Ready-to-edit samples ship in
[`cron/`](cron/): a [`crontab.example`](cron/crontab.example) and a
[launchd plist](cron/com.example.autonomous-prototyper.plist).

The two-phase gate keeps this clean: the ideator **ignores cron pokes
until `direction.md` exists**, so the clock only ever drives Phase (ii),
after you've kicked it off. You can install the cron from day one — a poke
before you've settled the direction is a harmless no-op.

**Two things to know before you run it 24/7** (both have knobs in
[`team-compose.yaml`](.team/team-compose.yaml)):

- **Cost.** Research cycles use provider tokens. Keep the cadence modest and use provider-side spending controls. `budget.daily_usd_limit` (set to `15.0` here) records an intended limit; TeamCTL does not enforce that USD ceiling today.
- **Pile-up.** `teamctl send` queues messages while the team is down. Garbage collection removes old acknowledged mail, not an unread backlog. Stop the scheduler when the team is stopped, and inspect queued messages before restarting it.

> **What TeamCTL does *not* do here:** there is no in-product scheduler,
> no self-rescheduling timer, no cadence field in the compose schema. The
> OS scheduler is the heartbeat. If you read the config expecting to find
> a built-in clock, this note is why you won't.

## The cross-model parity gap

One honest asymmetry. In this version, **`subagents:`, `skills:`, and
`hooks:` are claude-only** — declared on a Codex agent they render
nothing. So:

- The **ideator** and **pessimist** are Claude agents *because their value
  is their sub-agent stacks* — the research and the kill-stack only render
  on Claude. That's a design constraint, not a preference.
- The **prototyper** is **Codex** (the required cross-model mix). It runs
  lighter-stacked: its role prompt + native Codex tooling, but not the
  sub-agent / hook stack. For a *throwaway prototype builder*, fast native
  building is the right fit, so the gap costs little here. A Codex agent
  *can* still take a per-agent **`mcps:`** server (that field is
  runtime-agnostic — there's a commented example in
  [`projects/autonomous-prototyper.yaml`](.team/projects/autonomous-prototyper.yaml)).

When per-agent sub-agents/skills/hooks gain Codex support, the
prototyper's stack should be brought to parity.

## Install

```bash
# 1. Install teamctl + the runtimes this team uses.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code
# codex: see OpenAI's install docs (used by the prototyper)

# 2. Create ONE Telegram bot via @BotFather (for the ideator). Get your
#    chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/autonomous-prototyper ~/autonomous-prototyper
cd ~/autonomous-prototyper

# 4. Fill in the bot token + your chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env
```

## Run

```bash
# Run from the project root (where you copied the example to).
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

`teamctl up` also **starts the ideator's Telegram bot for you** — it
launches one `team-bot` (in its own tmux session) for each manager that
carries a `telegram:` block. Watch for the
`up · bot … → autonomous-prototyper:ideator` line. Don't launch a second
`team-bot` by hand: two pollers on one bot token collide (Telegram 409).

```bash
# If you see `skip · bot … TEAMCTL_TG_IDEATOR_TOKEN unset`, your token
# isn't in .team/.env yet. The easiest fix:
teamctl bot setup          # walks you through BotFather → token → chat id
teamctl up                 # then bring the bot up
teamctl bot status         # check it's connected
```

**Phase (i) — settle the direction.** DM the ideator what you're chasing.
It opens from `workspace/seed.md`, talks it through, and writes
`workspace/direction.md` when you agree. Nothing autonomous runs yet.

**Phase (ii) — start the heartbeat.** Install the cron so the hunt
actually runs on a cadence (edit the path inside first):

```bash
crontab cron/crontab.example
# macOS launchd alternative: see cron/com.example.autonomous-prototyper.plist
```

From here the ideator hunts on each poke, and surviving ideas arrive in
your chat as approve/reject taps.

## A note on "prototypes"

The prototyper builds **throwaway** prototypes — the smallest thing that
proves an idea's bet, with the hard parts stubbed and labeled. A vetted
idea is *researched and prototyped*, not shipped as a running startup in
one `teamctl up`. That's deliberate: it keeps the demo honest and
completing. The output is signal — "this bet holds up" — not a company.

## Point it at your own hunt

The startup-idea domain is the embodiment, not the point — the *loop* is.
To aim it at your own interests, edit one file:

- **[`workspace/seed.md`](workspace/seed.md)** → your own rough ideas or a
  starting direction. Then DM the ideator to settle it into a fresh
  `direction.md`.

The three-agent shape, both phases, the kill-stack, and the cron heartbeat
are unchanged — only what you're hunting moves. (Clear any old
`workspace/direction.md`, `workspace/ideas/`, and `workspace/prototypes/`
first so the team starts fresh.)

## Teardown

```bash
teamctl down
# Stop the heartbeat: remove the "run a cycle" line via `crontab -e`
# (or `launchctl unload` the launchd plist). Avoid `crontab -r` — it
# wipes your ENTIRE crontab, not just this job.
rm -rf .team/state/
```
