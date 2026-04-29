---
title: "Reference: `teamctl` CLI"
---

## Global flags

| Flag | Env | Default | Notes |
|---|---|---|---|
| `-C <path>` / `--root <path>` | `TEAMCTL_ROOT` | (auto) | Compose root (the directory holding `team-compose.yaml`). When unset, teamctl walks up from CWD looking for `.team/team-compose.yaml`. |

Resolution order for the root: `--root` > `TEAMCTL_ROOT` > current context (`teamctl context current`) > walk up from CWD.

Set `TEAMCTL_LOG=debug` for verbose tracing.

## Setup

| Command | Effect |
|---|---|
| `teamctl init [--template <name>] [--project <id>] [-y/--yes]` | Scaffold a fresh `.team/` directory in the current repo. Interactive by default; `--yes` accepts defaults. See [Init templates](#init-templates) below. |

### Init templates

`teamctl init` writes a `.team/` folder seeded from one of the bundled templates. Run interactively to pick from a menu, or pass `--template <key>`:

| Key     | Label      | What you get |
|---------|------------|--------------|
| `solo`  | Solo team  | One project, one manager, one dev worker, Claude Code on both. Ships a sample roles README and `.env.example`. Default if you pass `--yes` without `--template`. |
| `blank` | Blank      | Minimal `team-compose.yaml` + an empty `projects/main.yaml`. No agents seeded. For users who want to wire everything by hand. |

`--project <id>` overrides the auto-derived project id (the repo directory name). Re-running `init` in a directory that already has a `.team/` aborts to avoid clobbering work — delete or move it first.

## Lifecycle

| Command | Effect |
|---|---|
| `teamctl validate` | Parse the compose tree and check invariants. Exits non-zero on error. |
| `teamctl up` | Render artifacts, register agents in the mailbox, start every tmux session. Auto-registers the current root as a context on first run. |
| `teamctl down` | Stop every tmux session. State is preserved. |
| `teamctl reload` | Diff against the last-applied snapshot; restart only changed agents. |

## Inspection

| Command | Effect |
|---|---|
| `teamctl ps` (alias `status`) | Wide table: agents, supervisor state, inbox depth. |
| `teamctl logs <project>:<agent>` | Capture the tmux pane's scrollback (last ~3000 lines). |
| `teamctl tail <project>:<agent> [-f/--follow]` | Live message stream for an agent. |
| `teamctl mail [<project>:<agent>] [--all]` | Inbox snapshot for an agent (or `--all` across the team). |
| `teamctl inspect <project>:<agent>` | Full snapshot of an agent: env, mcp config, prompt, recent messages, today's costs. |

## Mailbox

| Command | Effect |
|---|---|
| `teamctl send <project>:<agent> "<text>"` | Inject a message into an agent's inbox as `sender=cli`. |

## Approvals (HITL)

| Command | Effect |
|---|---|
| `teamctl approvals` (alias `pending`) | Show pending HITL approval requests. |
| `teamctl approve <id> [--note "…"]` | Approve a pending request. |
| `teamctl deny <id> [--note "…"]` | Deny a pending request. |

## Bridges

| Command | Effect |
|---|---|
| `teamctl bridge open --from <p>:<m> --to <p>:<m> --topic "…" [--ttl <min>]` | Open a cross-project manager bridge. TTL defaults to 120 minutes. |
| `teamctl bridge close <id>` | Close a bridge early. |
| `teamctl bridge ls` (alias `list`) | List bridges with state (open / expired / closed). |
| `teamctl bridge log <id>` | Print the transcript for a bridge. |

## Budget / GC

| Command | Effect |
|---|---|
| `teamctl budget [--project <id>]` | Today's per-project activity + USD ledger. |
| `teamctl gc` | Delete acked messages past TTL; mark expired approvals. |

## Attach / exec

| Command | Effect |
|---|---|
| `teamctl attach <project>:<agent> [--rw]` | Attach to an agent's tmux session, read-only by default. `--rw` allows keyboard input and prompts before attaching. |
| `teamctl exec <project>:<agent> -- <argv...>` | Run a command in the agent's CWD with its env loaded. Use `--` to pass through hyphenated arguments. |
| `teamctl shell <project>:<agent>` | Open an interactive shell in the agent's CWD with its env loaded. |

## Environment

| Command | Effect |
|---|---|
| `teamctl env [--doctor]` | List the environment variables referenced by compose. `--doctor` flags missing or empty values. Does not require a resolved root. |

## Contexts

A *context* is a named pointer to a `.team/` root on this machine. Useful when you run more than one team and want to switch between them without `cd`-ing or passing `--root` every time.

| Command | Effect |
|---|---|
| `teamctl context ls` | List registered contexts. |
| `teamctl context current` | Print the active context name. |
| `teamctl context use <name>` | Set the active context. |
| `teamctl context add <name> <path>` | Register a new context. |
| `teamctl context rm <name>` | Remove a context. |

`teamctl up` auto-registers the current root as a context on first run.

## Rate-limit watcher

| Command | Effect |
|---|---|
| `teamctl rl-watch <project>:<agent> -- <runtime-command...>` | Wrap a runtime invocation, watching its output for rate-limit signatures. Used internally by `agent-wrapper.sh`; not normally invoked by hand. |

See the [Rate limits](/concepts/rate-limits/) concept for the hook chain that fires when a rate-limit is detected.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (or `validate`: no errors). |
| 1 | Validation errors; unknown agent; runtime failure. |
| 2 | Missing dependency (`tmux`, runtime binary). |
