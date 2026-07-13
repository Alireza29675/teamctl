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
| `teamctl adjust` | Confirm intent, then exec `claude /teamctl:adjust` to evolve an existing `.team/` (hire, retire, modify, bot-setup, add project, open a bridge). Interactive-only — `--yes` errors out; the skill collects what it needs from there. |

### Init templates

`teamctl init` writes a `.team/` folder seeded from one of the bundled templates. Run interactively to pick from a menu, or pass `--template <key>`:

| Key          | Label      | What you get |
|--------------|------------|--------------|
| `guided`     | Guided     | Ships no files. Confirms intent, then execs `claude /teamctl:init` so the LLM-led conversational setup takes over. Default when you run `teamctl init` interactively. Incompatible with `--yes` (the confirm step is interactive); the CLI errors clearly if you combine the two. |
| `essentials` | Essentials | Two projects out of the box: a blank `main` for the operator + an `ops` project with a single `builder` agent whose job is to scaffold and evolve `main` for you on Telegram. Ships a `README.md` walking through the bot setup. Default when you pass `--yes` without `--template`. |
| `blank`      | Blank      | Minimal `team-compose.yaml` + an empty `projects/main.yaml`. No agents seeded. For users who want to wire everything by hand. |

`--project <id>` overrides the auto-derived project id (the repo directory name). Re-running `init` in a directory that already has a `.team/` aborts to avoid clobbering work — delete or move it first.

## Lifecycle

| Command | Effect |
|---|---|
| `teamctl validate` | Parse the compose tree and check invariants. Exits non-zero on error. |
| `teamctl up [PROJECT] [--fresh]` | Render artifacts, register agents in the mailbox, start every tmux session. Auto-registers the current root as a context on first run. Pass an optional project name (`project.id` or filename stem) to scope to one project — scoped runs skip the cross-project DB rewrite and merge just that project's per-agent entries into `applied.json`. `--fresh` starts the agents this command actually spawns in a new conversation — see [Fresh restarts](#fresh-restarts). |
| `teamctl down [PROJECT]` | Stop every tmux session. State is preserved. Optional project name scopes to one project. |
| `teamctl reload [PROJECT] [--fresh]` | Diff against the last-applied snapshot; restart only changed agents. Optional project name applies the diff for that project only and merges its per-agent entries into `applied.json`, leaving other projects' last-applied state untouched. `--fresh` restarts the affected agents into a new conversation — see [Fresh restarts](#fresh-restarts). |

### Fresh restarts

`--fresh` (on both `up` and `reload`) brings the affected agents up in a **brand-new conversation** that re-runs their bootstrap prompt, instead of resuming the prior session. It is the escape hatch from always-on session resume — for a wedged context, a bad self-compact, or token bloat from a very long session.

It is **not** a file wipe: durable on-disk state (`task.md`, `memory/`, `ways-of-working.md`) is kept, so the agent re-grounds itself on restart. Under the hood teamctl moves the agent's Claude session log aside (preserved as a `.bak` recovery copy) so the next spawn starts fresh at the same deterministic session id. For `codex` agents it moves the per-agent `CODEX_HOME` sessions directory aside (preserved as a `sessions.bak` recovery copy, replacing any earlier one), so the wrapper's resume probe falls through to a fresh spawn; config and credentials at the codex-home root are untouched.

- **Scope.** `--fresh` only affects agents that are actually (re)started by the command. `up --fresh` freshens the agents it spawns; an already-running agent is left untouched (use `reload --fresh` to refresh one that's running). An unscoped `reload --fresh` with no config changes restarts nothing, so it is a no-op — use the scoped form `teamctl reload <PROJECT> <AGENT> --fresh` to fresh-restart specific agents whose config is unchanged.
- **Composition.** `--fresh` is orthogonal to the scoped force-restart and composes with it.
- **Runtime support.** `--fresh` covers Claude and `codex` agents. For `gemini` agents it is skipped with a warning rather than failing the rest of the team — the remaining parity gap, to close once that runtime's session model is mapped.

## Inspection

| Command | Effect |
|---|---|
| `teamctl ps [--json]` | Host-wide overview: every running team and its working directory, across projects. Runs from anywhere — no compose root required. |
| `teamctl status` | Wide table for the current team: agents, supervisor state, inbox depth. |
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
| `teamctl attach <project>:<agent> [--ro]` | Attach to an agent's tmux session, read-write by default (keystrokes reach the live agent). Pass `--ro` to attach read-only. (`--rw` is still accepted for back-compat but is now a no-op.) |
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
