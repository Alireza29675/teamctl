# Example: product-team

Product discovery while an engineering team builds. You set a goal, and
two managers you talk to on Telegram run with it: a **PM** who owns the
*what* (product discovery → a living `requirements.md`) and an **EM** who
owns the *how* (decompose, route, integrate, ship, gate). Behind them,
**two engineers on two model families** build the product and review each
other's PRs. You touch exactly two agents; everything between a settled
requirement and a shipped increment runs on its own.

```
        ┌─ pm  (Claude Opus)    ← Telegram: PM bot   · the *what*: discovery → requirements.md
You ────┤
        └─ em  (Claude Opus)    ← Telegram: EM bot   · the *how*: decompose, route, integrate, gate
                 │
                 ├─ eng-claude  (Claude Sonnet)  · builds slices · reviews eng-codex's PRs
                 └─ eng-codex   (Codex GPT-5)    · builds slices · reviews eng-claude's PRs

channels:   #product  pm ↔ em       #eng  em + engineers       #code_review  engineer ↔ engineer (cross-model)
```

The team operates on the bundled [`habit-tracker/`](habit-tracker/) seed
app — a tiny static habit/streak tracker it grows from skeleton toward
[`.team/requirements.md`](.team/requirements.md). Swap it for your own
product in one line (see [Point it at your own product](#point-it-at-your-own-product)).

## The autonomy loop

`requirements.md` is the contract between the two tracks. The PM writes
it; the EM delivers against it.

1. **You → PM** (Telegram): "build X" — the goal.
2. **PM** runs discovery (`product-researcher`), writes/maintains
   `requirements.md` (`prd-drafter`), and posts it to the EM on
   `#product`. It pings you *only* when a product decision is genuinely
   ambiguous and blocking.
3. **EM** reads the contract, decomposes it, and routes slices to the
   engineers on `#eng`. It integrates results and reports shippable
   increments to you.
4. **eng-claude / eng-codex** build their slices and review each other on
   `#code_review` — a cross-model pass before anything goes to a gate.
5. **EM** gates anything sensitive (`merge_to_main`, `release`, `deploy`,
   `publish`, `external_api_post`) → a Telegram approval prompt to you.
6. The loop continues: the PM keeps discovering and refining; you adjust
   the *what* anytime via the PM; the EM keeps shipping toward the current
   contract. You're never the bottleneck.

## What this demonstrates

Three properties make this a working team, not a toy:

1. **Per-agent capability stacks.** Every agent carries its own tooling,
   declared in `projects/product-team.yaml` — not a shared global set:
   - **pm** → `product-researcher`, `prd-drafter`
   - **em** → `code-investigator`, `pr-summarizer`
   - **eng-claude** → the full build stack (`code-investigator`,
     `implementer`, `test-author`, `qa-tester`, `pr-narrator`,
     `code-roaster`) **plus a `PreToolUse` fmt+lint hook** that gates
     every `Edit`/`Write` through `hooks/fmt-lint.sh`.
   - **eng-codex** → its role + native Codex tooling (see the parity note
     below).
2. **Cross-model PR review.** eng-claude (Claude) and eng-codex (Codex)
   review *each other's* PRs on `#code_review`. Two model families catch
   different bug classes — a genuine quality mechanism, not decoration.
3. **A deliberately narrow human surface.** You touch exactly two agents,
   on two separate bots: product with the PM, delivery with the EM.
   Everything between a requirement and a shipped increment is autonomous,
   with hard gates only where work reaches the outside world.

## The cross-model parity gap

Be aware of one honest asymmetry. In this version, **`subagents:`,
`skills:`, and `hooks:` are claude-only** — declared on a Codex (or
Gemini) agent they render nothing. So `eng-codex` runs *lighter-stacked*
by design: it gets its role prompt and native Codex tooling, but not the
sub-agent / hook stack that `eng-claude` carries.

`eng-codex` is **not** capability-zero, though — **`mcps:` is
runtime-agnostic**, so a Codex agent *can* take a per-agent MCP server
(there's a commented example in `projects/product-team.yaml`). And the
**cross-model review loop works fully regardless** — reviewing a PR needs
no sub-agents. When per-agent sub-agents/skills/hooks gain Codex support,
`eng-codex`'s stack should be brought to parity.

## Install

```bash
# 1. Install teamctl + the runtimes this team uses.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code
# codex: see OpenAI's install docs (used by eng-codex)

# 2. Create TWO Telegram bots via @BotFather — one for the PM, one for
#    the EM. Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/product-team ~/product-team
cd ~/product-team

# 4. Fill in the two bot tokens + your chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env
```

The seed app ships in `habit-tracker/` — no workspace to create. The team
reads and builds it via `cwd: ../habit-tracker` in
`projects/product-team.yaml`.

## Run

```bash
# Run from the project root (where you copied the example to).
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Then start the two manager bots (each in its own shell, or backgrounded).
The mailbox lives at `.team/state/mailbox.db` — its path is relative to the
compose file, not your shell:

```bash
# PM bot — the product conversation
team-bot \
  --mailbox ./.team/state/mailbox.db \
  --token   "$TEAMCTL_TG_PM_TOKEN" \
  --authorized-chat-ids "$TEAMCTL_TG_PM_CHATS" \
  --manager product-team:pm

# EM bot — the delivery conversation
team-bot \
  --mailbox ./.team/state/mailbox.db \
  --token   "$TEAMCTL_TG_EM_TOKEN" \
  --authorized-chat-ids "$TEAMCTL_TG_EM_CHATS" \
  --manager product-team:em
```

DM the **PM** the goal ("build a habit tracker — I want streaks"). It runs
discovery, writes `requirements.md`, and hands off to the EM. Delivery
updates and merge-gate approvals arrive in the **EM** chat.

## Point it at your own product

The habit tracker is a seed, not the point — the *team* is. To aim it at
your own repo, change one line and one file:

1. **`cwd`** in `.team/projects/product-team.yaml` → your project's path.
2. **`.team/requirements.md`** → your product's contract (clear the
   habit-tracker starter; the PM grows the rest from your goal).

The four-agent shape, the channels, and the per-agent stacks are
unchanged — only the target moves.

## Teardown

```bash
teamctl down
rm -rf .team/state/
```
