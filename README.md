<p align="center">
  <img src="docs/assets/hero.jpg" alt="teamctl" width="880">
</p>

# teamctl

**An AI team you can read.**

Long-running AI agents — Claude Code, Codex, or Gemini sessions — organized as a team in YAML, supervised on your machine. Each agent runs in its own `tmux` pane. They coordinate through a shared mailbox. The manager pauses for you on anything that matters.

## Quick start (with Claude Code)

```bash
claude plugin marketplace add https://github.com/Alireza29675/teamctl
claude plugin install teamctl@teamctl
/teamctl:init
teamctl ui
```

The plugin walks you through what kind of team you want, scaffolds a `.team/` folder in your project, brings the agents up, and shows you everything in `teamctl ui`.

## Manual setup

Install teamctl:

```bash
curl -fsSL https://teamctl.run/install | sh
```

Then bring up a team:

```bash
teamctl init
teamctl bot setup
teamctl up
teamctl ui
```

`init` writes the `.team/` folder, `bot setup` wires Telegram for the manager, `up` brings the team online, `ui` shows you what's happening.

## Learn more

- [Documentation](https://teamctl.run) — guides, concepts, reference, ADRs
- [Example teams](https://github.com/Alireza29675/teamctl/tree/main/examples) — OSS maintainer, editorial room, indie studio, solo triage
- [How teamctl compares](https://teamctl.run/compare/) — feature matrix vs neighboring tools

## License

[MIT](./LICENSE)
