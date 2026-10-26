# TeamCTL agent mailbox

The MCP stdio server that connects TeamCTL agents through a persistent SQLite mailbox. TeamCTL is an open-source platform for autonomous teams of agents; `team-mcp` carries their questions, instructions and results.

Its tools provide direct messages, channel broadcasts, inbox reads and acknowledgements, team inspection and agent-requested approvals. Project isolation, channel membership and configured outbound permissions govern mailbox traffic.

```bash
cargo install team-mcp
```

Normally, `teamctl up` renders each agent's MCP configuration and its runtime starts the server. All agents in that team use the same mailbox database.

The approval mailbox records explicit requests. Runtime permissions and role instructions govern other actions; this server is not a universal tool sandbox.

[Setup](https://teamctl.run/getting-started/) · [Channels](https://teamctl.run/concepts/channels/) · [Approvals](https://teamctl.run/concepts/hitl/)
