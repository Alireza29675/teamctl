---
title: Your first team
---

Define a team for your work, then start its agents in the background. Use guided setup or adapt a complete example.

You'll need macOS or Linux, `tmux`, `git`, and the runtimes you choose, installed and signed in: Claude Code, Codex, OpenCode or Gemini CLI. Keep the host awake while the team runs.

## Path 1: guided (recommended)

The Guided template opens the Claude Code plugin, so this path requires both `claude` and the [TeamCTL plugin](https://github.com/Alireza29675/teamctl/tree/main/plugins/claude-code):

```bash
cd /path/to/your/project
teamctl init --template guided
```

Describe your goal, review the proposed roles and structure, and choose when to bring the team up. The plugin writes editable YAML and Markdown under `.team/`. [Designing an agent team](/concepts/teams/) covers peer collaboration, leads and domain ownership.

## Path 2: start from a real example

Choose an example as a starting point, then review its roles, credentials and paths. The `personal-research` definition has a buddy and a curator, one project and Claude Code. You can also define peers without `reports_to` or adapt a larger organization:

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

Edit and share the configuration and role files. Document required tools, and leave credentials and private runtime state out of shared files.

Edit a role prompt and run `teamctl reload`: only the affected agents restart, others are untouched. Same for the project YAML; `reload` picks up adds, drops, and renames.

## What's next

- Add a [Telegram bot](/guides/telegram-bot/) so you can DM the manager from your phone.
- Read about [channels](/concepts/channels/) to wire up broadcast groups.
- See the bigger [`oss-maintainer`](https://github.com/Alireza29675/teamctl/tree/main/examples/oss-maintainer) and [`newsletter-office`](https://github.com/Alireza29675/teamctl/tree/main/examples/newsletter-office) examples for more developed shapes.
