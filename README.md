<h1 align="center">🫙 teamctl</h1>

<br/>

<p align="center">
  <strong>You build the perfect Claude Code setup, then it dies with the session.</strong><br/>
  <strong>You can't run it again, can't hand it to a teammate, can't pass it on.</strong><br/>
  <strong>teamctl lets you bottle it: your whole agent setup as something you</strong><br/>
  <strong>can run, share, and remix like a recipe.</strong>
</p>

<br/>

You describe your team in YAML: who the agents are, what each one owns, and how they talk to each other. Each agent is a real Claude Code, Codex, or Gemini session with its own memory. `teamctl up` brings them up. Hand it to a teammate and they get the same team, not a screenshot of yours.

I built this for myself. Experiments worth sharing, so here we are :)

## 🚀 Get started

```bash
curl -fsSL https://teamctl.run/install | sh
```

Installs `teamctl` in your command line. Then, inside your project directory:

```bash
teamctl init
```

`init` opens a short conversation that surfaces the domains in your work and proposes a team shape. You can let it design the team with you (guided), start from a small essentials scaffold, or take an empty tree and hand-wire it yourself. By the time you're done, `.team/team-compose.yaml` is on disk and the team is running in `tmux`.

## 🍒 Extras on top

Because teamctl runs your sessions, it can hand them tools they would not have on their own. A session can call `compact_self` to compact its own context and keep going, and that is just the start:

1. 🔀 **[Orchestration and a shared mailbox](https://teamctl.run/concepts/channels/):** agents coordinate and message each other through durable channels you can audit.
2. ⚙️ **[Per-agent settings](https://teamctl.run/reference/team-compose-yaml/):** give each agent its own runtime, model, role, and tools.
3. 🧬 **[Cascading role prompts](https://teamctl.run/reference/team-compose-yaml/):** layer a shared `_base.md` and group files like `_engineer.md` under each agent's own role, so the rules they share live in one place.
4. 🖥️ **[One terminal UI for the whole team](https://teamctl.run/reference/teamctl/):** watch every agent in one place with `teamctl ui`.
5. 📱 **[Easy Telegram hookup](https://teamctl.run/guides/telegram-bot/):** steer your team from your phone (more interfaces on the way).
6. ⏳ **(soon) Auto-recovery from rate limits:** teamctl can let your session know when its rate limit has cleared so it picks the work back up.

> *These extras are optional. They are here to help fill the gaps as you design different agent setups.*

## 🧩 Examples

Real teams running on teamctl. Copy any of them as a starting point.

| Example | What it does |
|---|---|
| 🏗️ **[product-team](examples/product-team/)** | A product squad: a PM and engineers coordinating around what to build and ship. |
| 🛰️ **[oss-maintainer](examples/oss-maintainer/)** | Runs a one-person open-source project: triage, bug-fix PRs, docs, and release proposals you approve. |
| 🧪 **[autonomous-prototyper](examples/autonomous-prototyper/)** | Comes up with ideas and prototypes them end to end. |
| 🌱 **[personal-research](examples/personal-research/)** | A reading buddy that holds your interests, plus a curator that follows the news and surfaces what matters. |
| 📈 **[market-analysts](examples/market-analysts/)** | A read-only research desk that backs financial decisions, with one analyst whose only job is to dissent. |
| 💼 **[job-finder](examples/job-finder/)** | Runs your job search: watches boards, aligns your CV to postings, and drafts cover letters you approve. |

More under [`examples/`](examples/).

## 🧱 What a team looks like

A project YAML with one manager and three workers (illustrative, not a full config):

```yaml
version: 2

project:
  id: service-desk

# 📡 Slack-like channels: agents in a channel can post messages and receive notifications
channels:
  - name: all
    members: "*"

  - name: dev                 # 🔀 the two executors review each other's PRs here
    members: [claude_exec, codex_exec]

managers:
  # 🛎️ your one manager: you chat with it on Telegram, it runs the show
  service_desk:
    display_name: "Service Desk"
    runtime: claude-code
    model: claude-sonnet-4-6
    role_prompt:               # 🧬 cascading roles: _base.md layers into every agent
      - roles/_base.md
      - roles/service_desk.md
    interfaces:
      telegram:               # 📱 tap to talk to your team from your phone
        bot_token_env: TEAMCTL_TG_TOKEN
        chat_ids_env: TEAMCTL_TG_CHATS

workers:
  # 🤖 a Claude executor: ships work and reviews the Codex executor's PRs
  claude_exec:
    display_name: "Executor (Claude)"
    runtime: claude-code
    model: claude-opus-4-8
    role_prompt:
      - roles/_base.md
      - roles/_engineer.md     # 🧬 group layer shared by the engineers
      - roles/executor.md
    reports_to: service_desk
    subagents:                # 🧩 give an agent its own sub-agents (claude-code)
      - agents/researcher.md

  # 🤖 a Codex executor: the other half of the review loop
  codex_exec:
    display_name: "Executor (Codex)"
    runtime: codex
    model: gpt-5-codex
    role_prompt:
      - roles/_base.md
      - roles/_engineer.md
      - roles/executor.md
    reports_to: service_desk

  # 🔭 autonomous discovery: keeps finding and prototyping ideas
  research:
    display_name: "Research and Discovery"
    runtime: claude-code
    model: claude-opus-4-8
    role_prompt:
      - roles/_base.md
      - roles/research.md
    reports_to: service_desk
    subagents:
      - agents/researcher.md
      - agents/code_search.md
```

> *All of this lives in a .team/ folder in your project, so you can read it, version it, and share it.*

Then:

```bash
teamctl validate    # check the YAML
teamctl up          # bring the team up
teamctl ui          # watch them work
teamctl status      # is everyone alive?
```

## 📚 Learn more

- 📖 [How to think about agent teams](https://teamctl.run/concepts/teams/) for the methodology behind team design.
- 📚 [Documentation](https://teamctl.run) for full docs, concepts, and reference.
- 🧪 [How teamctl compares](https://teamctl.run/compare/) for the feature matrix against neighboring tools.

## 🤝 Contributing

If anything feels missing or off, please open an issue or a PR. This is an experiment and there is plenty left to build, so help is genuinely welcome.

## ⚖️ License

[MIT](./LICENSE)
