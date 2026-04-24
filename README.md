# teamctl

**docker-compose for persistent AI agent teams.**

Declare a team of long-lived Claude Code, Codex CLI, or Gemini CLI sessions in YAML. They live in `tmux`, are supervised by `systemd` (Linux) or `launchd` (macOS) — or just `tmux` on any box — and talk to each other through a shared MCP mailbox. One manager per project chats with you on Telegram. Brand-sensitive actions pause for your approval.

```bash
curl -sSf https://teamctl.run/install | sh   # not yet live
teamctl init hello-team
teamctl up
```

## How it works

```mermaid
flowchart TB
    User(["👤 you"])
    Bot["team-bot (Telegram)"]
    User <--> Bot

    subgraph ProjA["Project A"]
        MgrA(["manager"])
        W1(["worker 1"])
        W2(["worker 2"])
        MgrA --- W1
        MgrA --- W2
    end

    subgraph ProjB["Project B"]
        MgrB(["manager"])
        W3(["worker"])
        MgrB --- W3
    end

    Mailbox[("team-mcp<br/>SQLite mailbox")]

    Bot <--> MgrA
    Bot <--> MgrB
    ProjA <--> Mailbox
    ProjB <--> Mailbox
    MgrA <-. bridge (opt-in, TTL) .-> MgrB
```

- **Every node is a real long-lived CLI session** — Claude Code, Codex, or Gemini — running in its own `tmux` pane. Not in-process roles.
- **The mailbox is the only shared memory.** Agents talk via `dm`, `broadcast`, `inbox_watch`, `list_team` MCP tools.
- **Projects are isolated.** A worker in Project A cannot reach Project B unless Alireza opens a bridge between the two managers.
- **Brand-sensitive actions pause.** Calls tagged `publish`, `release`, `deploy`, `payment`, … block on `request_approval` and surface in Telegram with Approve / Deny.

## Status

Early. v0.1 under active development — see [ROADMAP](./ROADMAP.md) and the [CHANGELOG](./CHANGELOG.md).

## What you get

- Persistent Claude Code / Codex / Gemini CLI sessions in `tmux`
- Real-time DMs and channels (SQLite-backed, sub-5 ms)
- Multi-project isolation with opt-in bridges
- Human-in-the-loop approvals for brand-sensitive actions
- Declarative YAML — change it, run `teamctl reload`, zero downtime

## Docs

- [Getting started](./docs/getting-started.md)
- [Concepts](./docs/concepts/) — projects, channels, runtimes, bridges, HITL
- [Reference](./docs/reference/) — `team-compose.yaml`, CLI, runtimes
- [Guides](./docs/guides/) — multi-runtime, Telegram bot, ops
- [ADRs](./docs/adrs/) — architectural decisions

## License

[MIT](./LICENSE)
