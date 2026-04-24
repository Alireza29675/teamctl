# Getting started

> This page is accurate for Phase 0 (scaffold only). Phase 1 populates the `hello-team` example with a working manager agent.

## Prerequisites

- Linux (Ubuntu 22.04+, Debian 12+, or Arch). macOS support lands in 0.2.
- `tmux`, `systemd --user`, `git`.
- A [Claude Code](https://code.claude.com/) install on `$PATH`. (Codex CLI and Gemini CLI are optional — see [Runtimes](./concepts/runtimes.md).)

## Install

```bash
# Coming in Phase 9:
curl -sSf https://teamctl.run/install | sh

# Today (from source):
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo install --path crates/teamctl
```

## Hello team

```bash
teamctl init hello-team    # Phase 1
cd hello-team
teamctl up
teamctl status
teamctl logs hello:manager
```

At this point your manager agent is running under `systemd --user` in a `tmux` session named `a-hello-manager`. It is idle, polling its inbox through the `team-mcp` MCP server.

Send it something:

```bash
teamctl send hello:manager "summarise the README of the current directory"
```

Tail the logs to watch it respond.

## Next

- [Concepts: Projects](./concepts/projects.md) — how teamctl isolates work.
- [Concepts: HITL](./concepts/hitl.md) — keeping the manager from shipping bad content.
- [Reference: team-compose.yaml](./reference/team-compose-yaml.md) — every field documented.
