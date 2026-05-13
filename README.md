<p align="center">
  <img src="docs/assets/hero.png" alt="teamctl" width="880">
</p>

# teamctl

**A team of AI agents, collaborating over a messaging system, all declared in YAML. Persistent and scalable.**

Each agent is a real Claude Code, Codex, or Gemini session with its own identity, its own memory, and a domain it owns end-to-end. They talk to each other through a durable mailbox. They collaborate the way real teammates do: handing off, pushing back, picking up where the other left off, asking for help when they need it, keeping the work moving while you sleep. You stay in the loop on what matters through Telegram, the TUI, or the CLI.

teamctl is built to scale agent-to-agent collaboration and solve bigger problems with better orchestration. Read [How to think about agent teams](https://teamctl.run/concepts/teams/) for the methodology. Or skip to the examples below.

## Install

```bash
curl -fsSL https://teamctl.run/install | sh
```

That's it. The installer puts `teamctl`, `team-mcp`, `team-bot`, and `teamctl-ui` on your `$PATH`, plus the Claude Code plugin if `claude` is detected.

## Start a team

Inside your project directory:

```bash
teamctl init
```

`teamctl init` walks you through a real conversation (not a template menu) that surfaces the *domains* in your work and proposes a team shape around them. By the time you're done, `.team/team-compose.yaml` is on disk and the team is running in `tmux`.

After running `teamctl init`, you'll be offered three options:

- **Guided.** Walks you through team design conversationally via Claude Code (opens `/teamctl:init`). Best when you want help thinking through the shape.
- **Essentials.** Scaffolds a starter team with a blank `main` project plus a `builder` agent who helps you evolve it. Best when you want a self-service helper bot from day one.
- **Blank.** Gives you an empty compose tree. Best when you know exactly the shape you want and prefer hand-wiring.

If you'd rather hand-author the team yourself, read the [handbook](https://teamctl.run/concepts/teams/) first. It's the cognitive frame the guided flow would have walked you through.

## Examples

These are real teams running on teamctl. Copy any of them as a starting point.

| Example | Agents | What they do |
|---|---|---|
| 🌱 **[Personal research](examples/personal-research/)** | 2 (buddy + curator) | The buddy holds your reading list and the compounding mental model of what you actually care about. The curator runs on a loop following the news on your declared interests and surfaces what matters. |
| 💼 **[Job finder](examples/job-finder/)** | 3 (lead + scout + matcher) | Scout watches job boards. Matcher does the deep CV-to-posting alignment. Lead handles your applications domain and talks to you on Telegram. Drafts cover letters in your voice; you tap to send. |
| 💰 **[Personal finance](examples/personal-finance/)** | 3 (books + tracker + analyst) | Books talks to you on Telegram. Tracker watches your accounts and surfaces anomalies. Analyst builds the long-arc patterns (weekly digests, savings rate, category trends). Read-only by design; nothing moves money without your tap. |
| 🛠️ **[Solo founder ops](examples/solo-founder-ops/)** | 4 (hub + research + inbox + analytics) | Hub holds the day picture. Research chases context. Inbox drafts replies and keeps the journal. Analytics watches your product metrics. The everything-that-is-not-building team. |
| 📬 **[Customer support](examples/customer-support/)** | 2 (triage + drafter) | Triage reads everything coming in and decides what gets your eyes. Drafter writes the reply in your voice; you tap to send. HITL on every external send. |
| 🛰️ **[OSS maintainer](examples/oss-maintainer/)** | 5 (maintainer + triage + bug_fix + docs + release_manager) | Triage labels new issues. Bug-fix opens PRs. Docs keeps the manual honest. Release-manager runs in plan-mode and proposes releases for your approval. You stay in the work only you can do. |

More examples live under [`examples/`](examples/), including the legacy `market-analysts` (advanced finance desk with plan-mode dissent) and the four classic shapes (`hello-team`, `indie-game-studio`, `solo-triage`, `newsletter-office`, `startup-team`).

## What a team looks like

A project YAML, with one manager and a couple of workers:

```yaml
version: 2

project:
  id: side-project

channels:
  - name: all
    members: "*"

managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    role_prompt: roles/manager.md
    interfaces:
      telegram:
        bot_token_env: TEAMCTL_TG_MANAGER_TOKEN
        chat_ids_env: TEAMCTL_TG_MANAGER_CHATS

workers:
  dev:
    runtime: codex
    model: gpt-5-codex
    role_prompt: roles/dev.md
    reports_to: manager

  researcher:
    runtime: claude-code
    model: claude-sonnet-4-6
    role_prompt: roles/researcher.md
    reports_to: manager
```

Then:

```bash
teamctl validate    # check the YAML
teamctl up          # bring the team up
teamctl ui          # watch them work
teamctl status      # is everyone alive?
```

You can attach to any agent's tmux pane to read along: `teamctl attach side-project:dev`. You can also DM them from the CLI: `teamctl send side-project:manager "what's on my plate?"`.

## What you get

- **Persistent agents.** Each one has identity, memory, and a domain. They survive reboots.
- **A messaging backbone.** Agents DM and broadcast through Slack-like channels; you can audit every line.
- **Human-in-the-loop (HITL) when it matters.** `publish`, `release`, `deploy`, `external_email`. Every sensitive action pauses for your tap on Telegram before it ships.
- **Multi-runtime.** Mix Claude Code, Codex, and Gemini in the same team. Different agents can use different keys for billing or rate-limit headroom.
- **Project isolation.** Run unrelated teams side-by-side without cross-talk. Bridge two projects only when you mean to.
- **Telegram, today.** Discord, email, and more on the way.

## Learn more

- 📖 [How to think about agent teams](https://teamctl.run/concepts/teams/) for the methodology, the two-gate framing, and what TeamOps means in practice.
- 📚 [Documentation](https://teamctl.run) for full docs, concepts, reference, and ADRs.
- 🧪 [How teamctl compares](https://teamctl.run/compare/) for the feature matrix vs neighboring tools. No put-downs.

## License

[MIT](./LICENSE)
