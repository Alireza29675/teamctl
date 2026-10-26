# TeamCTL CLI

Create autonomous teams of agents. TeamCTL is an open-source platform for defining roles, structure and communication, running the team in the background, and sharing its definition.

The `teamctl` binary validates configuration, starts and stops agent sessions, reloads changed roles, and provides inspection, messaging and approval commands. It supports Claude Code, Codex, OpenCode and Gemini CLI on macOS or Linux with `tmux`.

```bash
cargo install teamctl
teamctl init --template guided
```

Guided setup requires Claude Code and the [TeamCTL plugin](https://github.com/Alireza29675/teamctl/tree/main/plugins/claude-code). You can also hand-author or adapt a complete example. Install the required companion binaries and chosen runtimes using the [setup guide](https://teamctl.run/getting-started/).

Keep the host awake while agents work. Their roles, tools and runtime permissions determine behavior.

[Documentation](https://teamctl.run/getting-started/) · [CLI reference](https://teamctl.run/reference/teamctl/) · [Examples](https://github.com/Alireza29675/teamctl/tree/main/examples)
