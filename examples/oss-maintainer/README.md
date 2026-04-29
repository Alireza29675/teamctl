# Example: oss-maintainer

The team a one-person open-source project wishes it had: a **triage**
worker who labels new issues, a **bug_fix** worker (Codex) who opens
PRs, a **docs** worker who keeps the manual honest after merges, and a
**release_manager** who runs in plan-mode and proposes releases the
maintainer approves on Telegram. You — the maintainer — talk to one
bot and stay in the work that only you can do.

```
maintainer (Claude Opus)              ← Telegram: maintainer bot
  ├─ triage          (Claude Sonnet)  · #triage  — labels new issues
  ├─ bug_fix         (Codex GPT-5)    · #dev     — opens PRs
  ├─ docs            (Claude Sonnet)  · #all     — cross-checks docs
  └─ release_manager (Claude Opus,    · #release — proposes releases
                      plan-mode)                   (read-only)
```

`triage` and `bug_fix` are insulated from each other's channels — the
labelling chatter never bleeds into the patch flow, and vice versa.
The `release_manager` lives in `permission_mode: plan` — it can read
every PR and the changelog, but it cannot tag, push, or run
`cargo publish`. Its only output is a release plan, which becomes a
Telegram approval prompt.

## Install

```bash
# 1. Install teamctl + the runtimes you want.
curl -sSf https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code
# codex — see OpenAI's install docs (used by bug_fix)

# 2. Create one Telegram bot via @BotFather.
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/oss-maintainer ~/oss
cd ~/oss

# 4. Fill in token + chat id.
cp .env.example .env
$EDITOR .env

# 5. Workspace dir (where the agents read your project from).
mkdir -p workspace
# Tip: symlink your repo into ./workspace/ so the agents can read it,
# or clone it there.
```

## Run

```bash
set -a; . ./.env; set +a

teamctl validate
teamctl up
teamctl status
```

Now start the maintainer bot:

```bash
team-bot \
  --mailbox ./state/mailbox.db \
  --token   "$MAINTAINER_BOT_TOKEN" \
  --authorized-chat-ids "$MAINTAINER_CHAT_IDS" \
  --manager oss:maintainer
```

DM the maintainer bot when an issue lands — paste the URL and let
`triage` route it. The release plan will arrive in the same chat as a
Telegram approval prompt when `release_manager` thinks the milestone
is ready.

## What this demonstrates

A **pipeline workflow with cross-channel ACLs and plan-mode HITL on
release-critical actions**. Three patterns layer:

1. **Pipeline.** triage → bug_fix → docs is a directed workflow where
   each worker only sees the channel it owns. Nobody is on `#all`
   except the maintainer and the docs worker (so docs can post broad
   updates without spamming the patch flow).
2. **Cross-channel ACL boundary.** `triage`'s `#triage` and
   `bug_fix`'s `#dev` are deliberately disjoint. Triage hands off via
   DM to bug_fix; the patch discussion lives in `#dev` and never
   pollutes the labelling queue.
3. **Plan-mode on the trust-sensitive surface.** `release_manager`
   cannot mutate the repo. Its release plan is a proposal that
   becomes a Telegram approval prompt; the maintainer's tap is the
   audit trail.

The release plan template lives in `roles/release_manager.md` — it's
the thing that makes plan-mode useful instead of frustrating, because
it gives the agent a concrete artifact to produce.

## Shape of a typical week

1. Issues land. `triage` labels them in `#triage`, DMs `bug_fix` for
   confirmed bugs, replies + closes feature requests politely.
2. `bug_fix` reproduces, writes a regression test, opens a PR. Posts
   a short summary in `#dev`.
3. Maintainer reviews the PR (on GitHub) and merges. `docs` reads the
   merged diff, opens a follow-up doc PR if anything's now stale.
4. Once a milestone's worth of PRs has merged, `release_manager` posts
   a release plan to `#release` (version bump, changelog draft,
   pre-mortem, rollback strategy) and calls `request_approval`. The
   maintainer reads it on Telegram, taps ✅, and runs the tag + publish
   themselves.

## Teardown

```bash
teamctl down
rm -rf state/
```
