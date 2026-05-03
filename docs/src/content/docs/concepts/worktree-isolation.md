---
title: Per-session worktree isolation
---

Every agent in a teamctl team is its own focused session — its own context, its own attention, its own scratch space. v2-A makes that property structurally enforced: each agent's tmux session launches in its own **git worktree**, on its own **`agents/<agent-id>` branch**, so concurrent file mutations across sessions don't collide.

## Layout

When `worktree_isolation: true` is set, `teamctl up` derives one worktree per agent under `<root>/state/worktrees/<agent>/` rooted off the project's resolved `cwd`:

```
my-repo/
├── .git/
├── src/                              # your source tree
└── .team/
    ├── team-compose.yaml             # supervisor.worktree_isolation: true
    ├── projects/
    │   └── oss.yaml
    └── state/
        └── worktrees/
            ├── maintainer/           # branch agents/maintainer
            │   └── …                 # full working tree, isolated from peers
            ├── triage/               # branch agents/triage
            ├── bug_fix/              # branch agents/bug_fix
            ├── docs/                 # branch agents/docs
            └── release_manager/      # branch agents/release_manager
```

Each worktree is a real git working tree — `git status`, `git log`, branches, and merges all work normally. The branch namespace `agents/` keeps these out of the operator's own branch space, so your `main`, your feature branches, and your work-in-progress checkouts stay untouched no matter what the agents do.

## Schema

Project-level setting in `team-compose.yaml`:

```yaml
supervisor:
  type: tmux
  tmux_prefix: t-
  worktree_isolation: true   # opt in to per-session worktrees
```

Set explicitly. New teams scaffolded by `teamctl init` ship `worktree_isolation: true`; pre-v2-A teams keep their legacy single-cwd behaviour until they add the field.

Per-agent opt-out for advanced cases (e.g. plugging an externally-managed worktree into a session):

```yaml
managers:
  pm:
    runtime: claude-code
    cwd_override: ./long-lived-feature-branch
```

`cwd_override` wins over `worktree_isolation`. Relative paths resolve against `.team/`.

## Resolution rules

`teamctl` resolves an agent's tmux `cwd` in this order:

1. **`agent.cwd_override`** — explicit per-agent opt-out wins.
2. **`supervisor.worktree_isolation: true`** — `<root>/state/worktrees/<agent-id>/`, branched off the project's current HEAD as `agents/<agent-id>`.
3. **`worktree_isolation: false` or absent** — every agent shares the project's `cwd` (legacy behaviour). Field-absent emits a validate warning; field-explicit-false is silent.

## Coordination

Per-session isolation means workers don't fight each other for the file tree, but they still need to share work eventually. Coordination back to a shared branch uses the existing **HITL `merge_to_main` approval flow** — same shape as before, no new mechanism:

1. A worker commits to its `agents/<worker>` branch inside its own worktree.
2. When the work is ready to integrate, the worker calls `request_approval(action="merge_to_main")` (or a manager-scoped equivalent).
3. The manager reviews the worker's branch from inside its own worktree (`git log agents/<worker>`, `git diff`).
4. On approval, the manager merges: `git -C <manager-worktree> merge agents/<worker>`. The result lands on `agents/<manager>`.
5. For the change to reach the operator's actual default branch (`main`), the manager makes its own `request_approval(action="merge_to_main")` call. The operator approves on Telegram and runs the merge from `agents/<manager>` into `main` themselves.

The substrate enforces isolation; the HITL flow enforces the merge cadence. Both are surface for a single guarantee: the operator's `main` only moves when the operator says it does.

## `teamctl up` and `teamctl down`

`teamctl up` is idempotent on worktrees:

- First call creates each worktree with `git worktree add -b agents/<agent-id> <path>`.
- Subsequent calls (after `down`, after a host reboot) reuse existing worktrees and branches. Agents pick up where they left off — same `claude --continue` semantics teamctl already supports.

`teamctl down` preserves worktrees by default (matches the wider "state preserved across `down && up`" promise). If you genuinely want to reset:

```bash
teamctl down --clean-worktrees
```

This removes every `state/worktrees/<agent>/` directory and deletes the `agents/<agent-id>` branches via `git branch -D`. Destructive: any unmerged worker work is dropped. The flag is opt-in for that reason.

## Edge cases

**Project root isn't a git repo.** `git worktree add` requires one. When `worktree_isolation: true` is set explicitly and `project.cwd` isn't a git working tree, `teamctl validate` fails with the canonical message:

> `worktree_isolation requires a git repo at project.cwd (<path>). Run \`git init\` there or set \`supervisor.worktree_isolation: false\`.`

teamctl never auto-runs `git init`. Initializing a repo is a real decision (commit history starts here, this becomes a tracked thing); a tool that does it silently steals the moment from the operator.

**`worktree_isolation` field absent.** Pre-v2-A teams keep their existing single-cwd behaviour at runtime — the field is treated as **`false`** (legacy) until you opt in. `teamctl validate` emits a one-time warning nudging you to set the field explicitly: `worktree_isolation: true` to opt in to per-session worktrees, or `worktree_isolation: false` to silence the warning while keeping legacy. Standard deprecate → warn → opt-in → next-major-flips cadence; nothing about your team moves until you say it does. New teams scaffolded by `teamctl init` ship `worktree_isolation: true` already wired in.

**Worker uncommitted changes in `project.cwd`.** Agent worktrees are separate working trees; the operator's working tree is untouched.

**Two agents try to `merge_to_main` concurrently.** The existing HITL flow serializes via the operator's approval queue. No additional locking needed at the substrate.

## Why this is structural, not advisory

teamctl's positioning is *"the runtime that decomposes context across focused sessions"*. Without worktree isolation, that property would be honest in marketing prose but unenforced at runtime — exactly the prose-runtime drift class the substrate constraints exist to prevent. v2-A pulls the property down into the runtime: the YAML and the supervisor enforce it together, so the marketing copy that sells five focused sessions describes what the bits actually do.

See [ADR 0006](/adrs/0006-per-session-worktree/) for the substrate decision context.
