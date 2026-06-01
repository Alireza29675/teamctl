<p align="center">
  <strong>You build the perfect Claude Code setup, then it dies with the session.</strong><br/>
  <strong>You can't run it again, can't hand it to a teammate, can't pass it on.</strong><br/>
  <strong>teamctl lets you bottle it: your whole agent setup as something you</strong><br/>
  <strong>can run, share, and remix like a recipe.</strong>
</p>

You describe your team in YAML: who the agents are, what each one owns, and how they talk to each other. Each agent is a real Claude Code, Codex, or Gemini session with its own memory. `teamctl up` brings them up. Hand it to a teammate and they get the same team, not a screenshot of yours.

I built this for myself. Experiments worth sharing, so here we are :)

## Get started

```bash
curl -fsSL https://teamctl.run/install | sh
```

Installs `teamctl` in your command line. Then, inside your project directory:

```bash
teamctl init
```

`init` opens a short conversation that surfaces the domains in your work and proposes a team shape. You can let it design the team with you (guided), start from a small essentials scaffold, or take an empty tree and hand-wire it yourself. By the time you're done, `.team/team-compose.yaml` is on disk and the team is running in `tmux`.

## Examples

Real teams running on teamctl. Copy any of them as a starting point.

| Example | What it does |
|---|---|
| 🛰️ **[oss-maintainer](examples/oss-maintainer/)** | Runs a one-person open-source project: triage, bug-fix PRs, docs, and release proposals you approve. |
| 🌱 **[personal-research](examples/personal-research/)** | A reading buddy that holds your interests, plus a curator that follows the news and surfaces what matters. |
| 📈 **[market-analysts](examples/market-analysts/)** | A read-only research desk that backs financial decisions, with one analyst whose only job is to dissent. |
| 💼 **[job-finder](examples/job-finder/)** | Runs your job search: watches boards, aligns your CV to postings, and drafts cover letters you approve. |

More under [`examples/`](examples/).

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
