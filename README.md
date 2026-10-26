# teamctl: autonomous agents, one workspace

Define persistent agents in readable YAML. Give each one a workspace, tools, communication channels and named human interfaces. Start them in the background, then talk to them in your browser or on Telegram.

```sh
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo build -p teamctl -p team-mcp -p team-bot
export PATH="$PWD/target/debug:$PATH"
codex login                 # or choose an authenticated Claude Code / OpenCode runtime
teamctl up
teamctl serve
```

The checked-in team is one [personal agent](examples/personal-agent/): a durable notebook, task list and journal, with a bounded work turn every fifteen minutes. Your model account supplies inference. `teamctl down` stops the agent; its files and messages stay on disk. To start elsewhere, `teamctl init my-workspace --yes` creates the same personal recipe.

## A conversation with your whole team

`teamctl serve` opens a private, local messenger connected to every defined agent. Chat supports Markdown, images, files, audio recording and isolated interactive HTML. Search conversations, browse shared channels, review approvals, inspect tools and interface grants, follow activity/costs, or connect a workspace shell. Interactive tmux agents also accept console keystrokes. Native agents receive messages through Chat and expose their execution log.

Static assets ship inside the Rust binary. SQLite stores conversations; private local asset storage keeps uploads and agent attachments for thirty days by default. Set `web.cache_retention_days` to a different number or `null` to retain them. No Node server or hosted message service is needed. `teamctl ui` remains available for terminal inspection.

```yaml
# .team/projects/personal.yaml
version: 1
project:
  id: personal
  name: Personal workspace
  cwd: ../workspace
agents:
  companion:
    runtime: codex
    role_prompt: roles/companion.md
    execution:
      mode: native
      turn_timeout_seconds: 300
      wake_interval_seconds: 900
      sandbox: { type: host }
    can_dm: []
    can_broadcast: [notes]
channels:
  - name: notes
    members: "*"
```

Agents are peers. `reports_to` expresses a hierarchy when useful; `can_dm`, channel membership and `can_broadcast` express communication permissions. Human interfaces have stable instance IDs, names and descriptions. An agent can use two separate Telegram bots and the browser, discover its available interfaces, and reply to the originating conversation or choose a specific interface.

## Tools, authentication and execution

Assign per-agent MCP tools over stdio or HTTP with environment-variable references for secrets. Provider CLIs own model login; teamctl creates separate provider session/config state per agent. Host mode runs with your account's permissions. Optional Docker mode runs bounded turns on a shared team network with explicit work/credential mounts. See [native execution and containers](docs/src/content/docs/guides/native-agents.md) for the capability/authentication matrix and prerequisites.

The browser defaults to loopback and a per-launch access link. API, asset and console access require its bearer capability. Keep the printed link private. For access beyond your computer, forward the loopback port through an SSH tunnel. Direct network binding is rejected. HTML components run without parent-page access or external network requests.

## Guides

- [Personal agent and two Telegram bots](examples/personal-agent/README.md)
- [Browser workspace](docs/src/content/docs/guides/web-workspace.md)
- [Configuration and secret references](docs/src/content/docs/reference/team-compose-yaml.md)
- [Architecture](ARCHITECTURE.md) and [development checks](CONTRIBUTING.md)

This branch contains the new workspace implementation. Published release binaries may predate `serve`; build this source to try it.
