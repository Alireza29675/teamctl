---
title: "Reference: `teamctl` CLI"
---

## Global flags

| Flag | Env | Default | Notes |
|---|---|---|---|
| `-C <path>` / `--root <path>` | `TEAMCTL_ROOT` | `.` | Compose root (directory with `team-compose.yaml`). |

Set `TEAMCTL_LOG=debug` for verbose tracing.

## Commands

| Command | Effect |
|---|---|
| `teamctl validate` | Parse the compose tree and check invariants. Exits non-zero on error. |
| `teamctl up` | Render artifacts, register agents in the mailbox, start every tmux session. |
| `teamctl down` | Stop every tmux session. State is preserved. |
| `teamctl reload` | Diff against the last-applied snapshot; restart only changed agents. |
| `teamctl status [--project X]` | Table of agents with supervisor state and inbox depth. |
| `teamctl logs <project>:<agent>` | Capture the tmux pane's scrollback (last ~3000 lines). |
| `teamctl send <project>:<agent> "…"` | Inject a message as `sender=cli`. |
| `teamctl bridge open --from <p>:<m> --to <p>:<m> --topic "…" --ttl <min>` | Open a cross-project manager bridge. |
| `teamctl bridge close <id>` | Close a bridge early. |
| `teamctl bridge list` | List bridges with state (open / expired / closed). |
| `teamctl bridge log <id>` | Print the transcript for a bridge. |
| `teamctl pending` | Show pending HITL approvals. |
| `teamctl approve <id> [--note "…"]` | Approve a request. |
| `teamctl deny <id> [--note "…"]` | Deny a request. |
| `teamctl budget [--project X]` | Today's per-project activity + USD ledger. |
| `teamctl gc` | Delete acked messages past TTL; mark expired approvals. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (or `validate`: no errors). |
| 1 | Validation errors; unknown agent; runtime failure. |
| 2 | Missing dependency (`tmux`, runtime binary). |
