---
title: Your first team
---

Two paths into a running team: pick whichever fits how you work.

## Path 1: guided (recommended)

`teamctl init` walks you through a real discovery conversation (it runs the Claude Code plugin under the hood, so you need `claude` installed):

```bash
cd /path/to/your/project
teamctl init
```

It surfaces the *domains* in your work, proposes a team shape around them, scaffolds `.team/` to disk, and brings the team up. Read [How to think about agent teams](/concepts/teams/) for the methodology the conversation is built on.

## Path 2: start from a real example

Pick the example closest to your work, copy it, edit. The `personal-research` example is a small one to start from: two agents (a buddy and a curator), one project, Claude Code:

```bash
git clone git@github.com:Alireza29675/teamctl.git   # the curl install doesn't bundle the examples
cp -r teamctl/examples/personal-research ~/my-team
cd ~/my-team
teamctl validate           # ok · 1 project · 2 agent sessions
teamctl up                 # renders .team/state/, starts tmux sessions
teamctl status             # shows the buddy and curator running
teamctl ui                 # watch them work
```

Send the agent a message:

```bash
teamctl send research:buddy "what's on my plate?"
```

## What `.team/state/` holds

Whichever path you took, `teamctl up` renders these under `.team/state/`:

- `envs/<project>-<agent>.env`: environment for the agent wrapper
- `mcp/<project>-<agent>.json`: MCP config pointing at `team-mcp`
- `mailbox.db`: SQLite mailbox (WAL mode)

## Change something

Edit a role prompt and run `teamctl reload`: only the affected agents restart, others are untouched. Same for the project YAML; `reload` picks up adds, drops, and renames.

## What's next

- Add a [Telegram bot](/guides/telegram-bot/) so you can DM the manager from your phone.
- Read about [channels](/concepts/channels/) to wire up broadcast groups.
- See the bigger [`oss-maintainer`](https://github.com/Alireza29675/teamctl/tree/main/examples/oss-maintainer), [`market-analysts`](https://github.com/Alireza29675/teamctl/tree/main/examples/market-analysts), and [`newsletter-office`](https://github.com/Alireza29675/teamctl/tree/main/examples/newsletter-office) examples for more developed shapes.
