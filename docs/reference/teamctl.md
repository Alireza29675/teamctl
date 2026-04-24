# Reference: `teamctl` CLI

> Stub. Populated alongside Phase 1.

| Command | Effect |
|---|---|
| `teamctl validate [path]` | Parse the compose tree and check invariants. |
| `teamctl up` | Start the fleet. |
| `teamctl down` | Stop the fleet. State is preserved. |
| `teamctl reload` | Apply compose diffs. Restarts changed agents only. |
| `teamctl status [--project X]` | Table of agents and inbox depth. |
| `teamctl logs <project>:<agent> [-f]` | Tail journal + per-agent JSONL. |
| `teamctl attach <project>:<agent>` | Read-only tmux attach. |
| `teamctl send <project>:<agent> "…"` | Debug: inject a message as `sender=cli`. |
| `teamctl bridge open/close/list/log` | Manage inter-project bridges. |
| `teamctl approve <id>` / `teamctl deny <id>` | CLI alternative to Telegram approvals. |
| `teamctl budget [--project X]` | Today's token spend. |
