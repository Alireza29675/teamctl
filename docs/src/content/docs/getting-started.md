---
title: Getting started
---

If you're hand-authoring `team-compose.yaml` and skipping interactive `init`, read [How to think about agent teams](/concepts/teams/) first: it's the cognitive frame the `init` flow would have walked you through.

## Prerequisites

- Linux (Ubuntu 22.04+, Debian 12+, Arch) or macOS.
- `tmux`, `git`.
- The runtime(s) you plan to use on `$PATH`:
  - [Claude Code](https://code.claude.com/): `claude`
  - [Codex CLI](https://openai.com/codex): `codex`
  - [Gemini CLI](https://ai.google.dev/gemini-cli): `gemini`

## Install

```bash
curl -fsSL https://teamctl.run/install | sh
```

That installs `teamctl`, `team-mcp`, `team-bot`, and `teamctl-ui` on your `$PATH`. If `claude` is detected, the installer also offers to install the Claude Code plugin in the same step.

From source if you'd rather:

```bash
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo install --path crates/teamctl
cargo install --path crates/team-mcp
cargo install --path crates/team-bot    # only if you want the Telegram adapter
```

## Your first team

The fastest path is the guided one. `teamctl init` runs a discovery conversation that surfaces the *domains* in your work and proposes a team shape around them:

```bash
cd /path/to/your/project
teamctl init
```

Read [How to think about agent teams](/concepts/teams/) first if you want the cognitive frame the guided flow is built on.

### Or hand-author

If you'd rather skip the wizard, start from a real example. Pick the shape closest to your work:

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

## Teardown

```bash
teamctl down            # stop tmux sessions; state preserved
rm -rf state/           # full reset
```

## Next

- [How to think about agent teams](/concepts/teams/): the methodology
- [Concepts · Projects](/concepts/projects/): how project isolation works
- [Concepts · HITL](/concepts/hitl/): keeping agents from shipping bad content
- [Reference · team-compose.yaml](/reference/team-compose-yaml/): the full schema
- [Guide · Telegram bot](/guides/telegram-bot/): wiring managers to chat
