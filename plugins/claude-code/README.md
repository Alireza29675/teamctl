# TeamCTL for Claude Code

Define and adjust your team through a conversation in Claude Code. The TeamCTL plugin writes the same YAML and Markdown you can edit by hand: roles, structure and communication under `.team/`.

TeamCTL is an open-source platform for autonomous teams of agents. A team can be research partners, a lead with builders and reviewers, or your own organization. The chosen runtimes and role instructions shape how it works.

## Install the plugin

Install TeamCTL and Claude Code **2.1.141 or newer**, then sign in to Claude Code. The TeamCTL installer can offer the plugin in an interactive terminal. To install it directly:

```bash
claude plugin marketplace add Alireza29675/teamctl --scope user
claude plugin install teamctl@teamctl --scope user
```

The marketplace and plugin identifiers stay `teamctl`; the display name is TeamCTL.

## Define or adjust a team

Two slash commands ship with this plugin:

- `/teamctl:init` — discuss the goal, review the proposed setup, write `.team/`, and choose when to start it. Resumable setup skips stages already completed.
- `/teamctl:adjust` — evolve an existing team. Add or remove an agent, change a role, scope a channel or configure Telegram through reviewed file edits.

From a terminal, `teamctl init --template guided` opens guided setup and `teamctl adjust` opens adjustment. These guided commands require Claude Code and this plugin. Teams themselves can use Claude Code, Codex, OpenCode or Gemini CLI.

Both commands write to `.team/` in your project. Read the files, validate changes with `teamctl validate`, and reload a running team with `teamctl reload`.

## Run and share the definition

Agents run in background `tmux` sessions on macOS or Linux. Keep the host awake and the chosen runtimes installed, signed in and permitted to do the work. Roles and runtime permissions govern behavior; the approval mailbox records requests agents explicitly make.

Share intended configuration, role and capability files with their prerequisites. Each user supplies their own accounts and permissions. Keep `.env`, private messages and runtime state out of shared files.

See [RULES.md](./RULES.md) for the guardrails both commands honour, and [role-prompt-style.md](./role-prompt-style.md) for the spine every generated role prompt follows.

[Documentation](https://teamctl.run/getting-started/) · [Team design](https://teamctl.run/concepts/teams/) · [Examples](https://github.com/Alireza29675/teamctl/tree/main/examples)
