---
title: Getting started
---

## Prerequisites

- Linux (Ubuntu 22.04+, Debian 12+, Arch) or macOS.
- `tmux`, `git`.
- The runtime(s) you plan to use on `$PATH`:
  - [Claude Code](https://code.claude.com/) — `claude`
  - [Codex CLI](https://openai.com/codex) — `codex`
  - [Gemini CLI](https://ai.google.dev/gemini-cli) — `gemini`

## Install

```bash
# Prebuilt binaries:
curl -sSf https://teamctl.run/install | sh

# From source:
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo install --path crates/teamctl
cargo install --path crates/team-mcp
cargo install --path crates/team-bot    # only if you want the Telegram adapter
```

## Your first team

Copy the `hello-team` example from the repo into a fresh directory and point `teamctl` at it:

```bash
cp -r teamctl/examples/hello-team ~/my-team
cd ~/my-team
teamctl validate
teamctl up
teamctl status
```

You now have two Claude Code sessions running in `tmux` — one manager, one dev — talking through a SQLite mailbox. Send the manager a message:

```bash
teamctl send hello:manager "summarise this directory"
teamctl logs hello:manager
```

## Teardown

```bash
teamctl down            # stop tmux sessions; state preserved
rm -rf state/           # full reset
```

## Next

- [Concepts · Projects](/concepts/projects/)
- [Concepts · HITL](/concepts/hitl/) — how to keep agents from shipping bad content
- [Reference · team-compose.yaml](/reference/team-compose-yaml/)
- [Guide · Telegram bot](/guides/telegram-bot/)
