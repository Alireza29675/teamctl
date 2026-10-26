# Example: newsletter-office

One possible TeamCTL team definition: a newsroom and a website team in separate projects. A **head editor** coordinates research and writing, then exchanges messages with the website manager through an operator-opened bridge.

This example declares a future email interface; that adapter does not ship yet. Use the host's messaging and approval commands today. Review the roles, tools and publishing permissions before running the definition.

Two projects, seven agents across three runtimes.

```
├─ newsroom            (runtimes: Claude Code + Gemini)
│   ├─ head_editor     · Claude Opus   · email interface
│   ├─ news_writer     · Claude Sonnet
│   ├─ fact_checker    · Gemini 3.0 Pro
│   └─ seo_research    · Gemini 3.0 Pro
└─ blog-site           (runtimes: Claude Code + Codex)
    ├─ web_manager     · Claude Opus
    ├─ frontend_dev    · Codex GPT-5
    └─ backend_dev     · Codex GPT-5
```

## Install

```bash
# 1. Install teamctl + the runtimes you need.
curl -sSf https://teamctl.run/install | sh         # teamctl, team-mcp, team-bot
npm i -g @anthropic-ai/claude-code                 # claude
# codex  — see OpenAI's install docs
# gemini — see Google's install docs

# 2. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/newsletter-office ~/newsroom
cd ~/newsroom

# 3. Seed credentials.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 4. Workspace dirs for the two projects' CWDs.
mkdir -p newsroom-workspace blog-site-workspace
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

The seven agents are now running in `tmux` sessions named
`news-newsroom-<agent>` and `news-blog-site-<agent>`.

## Email interface

The `email` interface type is declared in `.team/team-compose.yaml` and scoped
to `newsroom:head_editor`. The email adapter is planned
and does not ship today — drive the head
editor with:

```bash
teamctl send newsroom:head_editor "Topic: EU AI Act enforcement"
```

The declaration records the intended email connection. Revisit its schema and setup when an adapter ships rather than assuming this example already provides email delivery.

## Publishing a post

The head editor calls `request_approval(action="publish")` before any post
goes live. You'll see it in `teamctl pending`:

```bash
teamctl pending
teamctl approve 1 --note "lgtm"
```

## Cross-team handoff

When the head editor is ready to hand a post to the website team,
you (not an agent) open a bridge:

```bash
teamctl bridge open \
  --from newsroom:head_editor \
  --to   blog-site:web_manager \
  --topic "schedule: EU AI Act post" \
  --ttl 60

teamctl bridge log 1      # watch the conversation
teamctl bridge close 1    # close early if done
```

## Teardown

```bash
teamctl down
rm -rf state/
```
