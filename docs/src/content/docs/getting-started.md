---
title: Getting started
---

Create a team around the work you want agents to do together. TeamCTL provides the sessions and messaging; you define the roles, structure and conversations.

Start with guided setup or adapt an [example team](https://github.com/Alireza29675/teamctl/tree/main/examples). A research pair and an organization with leads and specialists are both possible configurations.

## Prerequisites

- Linux (Ubuntu 22.04+, Debian 12+, Arch) or macOS.
- `tmux`, `git`.
- The runtime(s) you plan to use on `$PATH`:
  - [Claude Code](https://code.claude.com/): `claude`
  - [Codex CLI](https://openai.com/codex): `codex`
  - [OpenCode](https://opencode.ai/): `opencode`
  - [Gemini CLI](https://ai.google.dev/gemini-cli): `gemini`

Install and sign in to each runtime you choose. Guided setup also requires Claude Code and the [TeamCTL plugin](https://github.com/Alireza29675/teamctl/tree/main/plugins/claude-code).

## Install

```bash
curl -fsSL https://teamctl.run/install | sh
```

That installs `teamctl`, `team-mcp`, `team-bot`, and `teamctl-ui` into `~/.local/bin`. Add that directory to your `$PATH` if needed. An interactive installer can offer the Claude Code plugin when a supported `claude` is detected; a piped install skips that prompt. See the [plugin setup](https://github.com/Alireza29675/teamctl/tree/main/plugins/claude-code) if it is not installed.

From source if you'd rather:

```bash
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo install --path crates/teamctl
cargo install --path crates/team-mcp
cargo install --path crates/team-bot    # only if you want the Telegram adapter
```

## Your first team

Choose the Guided template to open the Claude Code plugin. Describe your goal, review the proposed roles and structure, and choose when to start the team:

```bash
cd /path/to/your/project
teamctl init --template guided
```

The result is editable YAML and Markdown under `.team/`. Read [Designing an agent team](/concepts/teams/) for ways to choose the boundaries.

### Or hand-author

Choose one example, copy it, then review its roles, runtimes, required credentials and local paths before starting it:

```bash
# grab the examples once (the curl install doesn't bundle them)
git clone git@github.com:Alireza29675/teamctl.git

cp -r teamctl/examples/personal-research ~/my-team   # 2 agents
cp -r teamctl/examples/job-finder        ~/my-team   # 3 agents
cp -r teamctl/examples/oss-maintainer    ~/my-team   # 5 agents
cd ~/my-team
teamctl validate
teamctl up
teamctl status
teamctl ui                                           # watch them work
```

Send the manager a message:

```bash
teamctl send research:buddy "what's on my plate?"
```

## Keep the team running

Agents continue in the background when you close the dashboard. The host must stay awake, with `tmux` sessions running and provider access available. Their roles, tools and permissions determine what work they can do; starting a team does not guarantee an outcome.

Share the intended configuration and role files so others can run and adapt your setup. Each person supplies their own accounts and credentials. Keep `.env`, private messages and runtime state out of shared files.

## Teardown

```bash
teamctl down            # stop tmux sessions; state preserved
rm -rf state/           # full reset
```

## Next

- [Designing an agent team](/concepts/teams/): peers, leads and domain boundaries
- [Concepts · Projects](/concepts/projects/): how project isolation works
- [Concepts · HITL](/concepts/hitl/): agent-requested approvals and permission boundaries
- [Reference · team-compose.yaml](/reference/team-compose-yaml/): the full schema
- [Guide · Telegram bot](/guides/telegram-bot/): wiring managers to chat
