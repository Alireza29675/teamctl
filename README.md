<p align="center">
  <img src="docs/assets/hero.jpg" alt="teamctl — agentic organizations as code" width="880">
</p>

# teamctl

**docker-compose for persistent AI agent teams.**

Declare a team of long-lived Claude Code, Codex CLI, or Gemini CLI sessions in YAML. Every agent is its own real CLI running in its own `tmux` pane, supervised and auto-respawned (`systemd` and `launchd` backends are on the [ROADMAP](./ROADMAP.md)). They coordinate through a shared MCP mailbox. Each project has its own private org-chart with one or more managers; you talk to those managers over pluggable **interfaces** (Telegram, Discord, iMessage, CLI, webhook). Brand-sensitive actions pause for your approval.

```bash
curl -fsSL https://teamctl.run/install | sh
teamctl init hello-team
teamctl up
```

> Prefer to build from source? `cargo install teamctl team-mcp team-bot` works too. A Homebrew tap is on the way — see the [ROADMAP](./ROADMAP.md).

## What's in a team

- **Every node is a separate long-lived CLI** — Claude Code, Codex, or Gemini — running in its own `tmux` pane. No shared process, no "roles inside one LLM."
- **Projects are self-contained org charts.** One project can have many managers and many workers; workers are wired to one or more managers through `reports_to`. Agents can call `org_chart` to introspect their chain of command.
- **Managers talk to each other** inside a project (shared `#leads` channel or DM). Across projects they're isolated — a one-off **bridge** opens a manager-to-manager link for a limited time.
- **You reach managers through any of the configured interfaces.** Telegram is the first adapter; Discord, iMessage, CLI, and webhooks plug in the same way.
- **Brand-sensitive actions pause.** Tool calls tagged `publish`, `release`, `deploy`, `payment`, … block on `request_approval` and surface on your chosen interface with Approve / Deny.

## Status

Early but moving fast. v0.2.9 is the latest release (April 2026); MIT-licensed, working, and shipped in the open. See the [ROADMAP](./ROADMAP.md) and [CHANGELOG](./CHANGELOG.md) for the full picture.

## What you get

- Persistent Claude Code / Codex / Gemini CLI sessions in `tmux`, supervised and auto-respawned
- Real-time DMs, channels, and a per-agent inbox (SQLite-backed)
- MCP `notifications/claude/channel` events so Claude Code agents wake on new mail without polling
- `reply_to_user` so managers can talk back through the configured interface (Telegram today)
- Multi-project isolation with opt-in, time-boxed manager bridges
- Human-in-the-loop approvals for brand-sensitive actions, surfaced on Telegram with inline Approve/Deny
- Per-runtime rate-limit detection with a configurable hook chain (`wait` / `send` / `webhook` / `run`)
- Declarative YAML — change it, run `teamctl reload`, zero downtime
- A growing inspection toolbox: `teamctl ps / mail / tail / inspect / attach / exec / shell`

## How it compares

The space of "process-level supervisors of CLI coding agents" is busy, and the comparison most readers want is to other neighbors. The table below is a feature matrix, not a leaderboard — every row in this neighborhood does something well that teamctl doesn't, and vice versa.

| Feature                              | teamctl | claude-squad | claude-flow | mcp_agent_mail | raw `tmux + scripts` |
|--------------------------------------|:-------:|:------------:|:-----------:|:--------------:|:--------------------:|
| Declarative team file (YAML)         |   ✅   |      —       |      —      |       —        |          —           |
| Org charts as code (`reports_to`)    |   ✅   |      —       |      —      |       —        |          —           |
| Multi-runtime out of the box (Claude Code · Codex · Gemini) | ✅ | partial | Claude-only | n/a | DIY |
| Persistent agents across reboots     |   ✅   |   partial    |   partial   |     n/a        |         DIY          |
| Mailbox bundled with the supervisor  |   ✅   |      —       |      —      |   ✅ (alone)   |          —           |
| Inter-agent DMs + channels           |   ✅   |      —       |   partial   |      ✅        |         DIY          |
| Project isolation + opt-in bridges   |   ✅   |      —       |      —      |       —        |          —           |
| Telegram (or other) approval inbox (HITL) | ✅ |      —       |      —      |       —        |          —           |
| Service-grade supervision (tmux today; systemd · launchd planned) | ✅ | tmux | tmux | n/a | tmux |
| Single static binary, no runtime deps |   ✅   |      —       |      —      |       —        |         n/a          |

A few honest notes on the table:

- **claude-squad** is excellent at the "multiple sessions for one operator" job. It isn't trying to be a team framework, and that's fine.
- **claude-flow** is the largest project in the neighborhood and goes deep on swarms + neural patterns inside Claude. teamctl is narrower: persistent declarative teams, runtime-agnostic.
- **mcp_agent_mail** is an unbundled mailbox you wire into other tools. teamctl bundles a mailbox with the supervisor + interfaces — different layer, complementary problem.
- **`tmux + scripts`** is the honest baseline. If you'd rather hand-roll a wrapper script and a `mailbox.sh`, you can. teamctl is what happens after you've done that twice.

Sources: each cell is from the project's own README at the time of writing — happy to take corrections via PR.

## Docs

- [Getting started](./docs/getting-started.md)
- [Concepts](./docs/concepts/) — projects, channels, runtimes, bridges, HITL
- [Reference](./docs/reference/) — `team-compose.yaml`, CLI, runtimes
- [Guides](./docs/guides/) — multi-runtime, Telegram bot, ops
- [ADRs](./docs/adrs/) — architectural decisions

## License

[MIT](./LICENSE)
