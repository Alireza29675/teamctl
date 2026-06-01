<p align="center">
  <img src="docs/assets/hero.png" alt="teamctl" width="880">
</p>

<p align="center">
  <strong>You build the perfect Claude Code setup, then it dies with the session.</strong><br/>
  <strong>You can't run it again, can't hand it to a teammate, can't pass it on.</strong><br/>
  <strong>teamctl lets you bottle it: your whole agent setup as something you</strong><br/>
  <strong>can run, share, and remix like a recipe.</strong>
</p>

You describe your team in YAML: who the agents are, what each one owns, and how they talk to each other. Each agent is a real Claude Code, Codex, or Gemini session with its own memory. `teamctl up` brings them up. Hand it to a teammate and they get the same team, not a screenshot of yours.

I built this for myself. Some experiments are worth sharing, so here we are :)

## Install

```bash
curl -fsSL https://teamctl.run/install | sh
```

Puts `teamctl`, `team-mcp`, `team-bot`, and `teamctl-ui` on your `$PATH`, plus the Claude Code plugin if `claude` is detected.

## Start a team

Inside your project directory:

```bash
teamctl init
```

`init` opens a short conversation that surfaces the domains in your work and proposes a team shape. You can let it design the team with you (guided), start from a small essentials scaffold, or take an empty tree and hand-wire it yourself. By the time you're done, `.team/team-compose.yaml` is on disk and the team is running in `tmux`.

## Examples

Real teams running on teamctl. Copy any of them as a starting point.

| Example | Agents | What they do |
|---|---|---|
| 🌱 **[hello-team](examples/hello-team/)** | 2 | The smallest useful team: one manager and one dev talking through a shared SQLite mailbox. |
| 🛰️ **[oss-maintainer](examples/oss-maintainer/)** | 5 | Runs a one-person open-source project: triage, bug-fix PRs, docs, and release proposals you approve. |
| 🧰 **[solo-triage](examples/solo-triage/)** | 3 | Fields what's on your plate, chases context across the web and your docs, and drafts your replies. |
| 🛠️ **[solo-founder-ops](examples/solo-founder-ops/)** | 4 | Handles a founder's everything-else: research, inbox drafts, and product metrics. |
| 📬 **[customer-support](examples/customer-support/)** | 2 | Runs a support inbox: routes tickets and drafts voice-matched replies you approve before sending. |
| 📈 **[market-analysts](examples/market-analysts/)** | 5 | A read-only research desk that backs financial decisions, with one analyst whose only job is to dissent. |
| 🎮 **[indie-game-studio](examples/indie-game-studio/)** | 4 | A solo game dev's brain trust: vision, mechanics, narrative, and an honest playtest critic. |
| 🗞️ **[newsletter-office](examples/newsletter-office/)** | 7 | A newsroom that publishes a daily digest plus a web team that owns the blog, across three runtimes. |
| 🚀 **[startup-team](examples/startup-team/)** | 5 | A small startup shape where you talk to both a founder and a PM, who coordinate the engineers. |

More live under [`examples/`](examples/): `job-finder` (3), `market-analysis` (4), `personal-finance` (3), `personal-research` (2), and `gastown-in-teamctl` (7), a style example rather than a runnable port.

## What a team looks like

A project YAML with one manager and two workers (illustrative, not a full config):

```yaml
version: 2

project:
  id: my-project

channels:
  - name: all
    members: "*"

managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
    role_prompt: roles/manager.md

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

## What you get

- **Persistent agents.** Each one has identity, durable memory, and a domain it owns. Their conversation state survives process crashes and restarts: bring the team back up with `teamctl up` and they resume where they left off.
- **A messaging backbone.** Agents DM each other and broadcast through channels, and you can audit every line.
- **Human-in-the-loop when it matters.** Agents are configured to route sensitive actions (publish, release, deploy, external email) through an approval gate that waits for your tap before proceeding. Enforced through agent role prompts and the `request_approval` tool, not by intercepting tool calls.
- **Multi-runtime.** Mix Claude Code, Codex, and Gemini in one team, each agent with its own keys. Claude Code currently has the most complete feature set (session resume, `--fresh`), with Codex and Gemini at lighter parity.
- **Project isolation.** Run unrelated teams side-by-side without cross-talk, and bridge two projects only when you mean to.
- **Reach them where you are.** Watch and steer through the CLI, the TUI, or Telegram. (Telegram is the only adapter shipped today; Discord, email, and more on the way.)

## Learn more

- 📖 [How to think about agent teams](https://teamctl.run/concepts/teams/) for the methodology behind team design.
- 📚 [Documentation](https://teamctl.run) for full docs, concepts, and reference.
- 🧪 [How teamctl compares](https://teamctl.run/compare/) for the feature matrix against neighboring tools.

## License

[MIT](./LICENSE)
