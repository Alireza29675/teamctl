---
title: ADR 0006 — Per-session worktree as runtime primitive
---

- Status: **accepted** (implemented in v2-A)
- Date: 2026-05-03
- Author: dev2
- Reviewers: eng_lead, pm, project owner

## Context

Pre-v2-A every agent in a project shared `project.cwd`. tmux launched all agents with the same `-c <cwd>` argument, and the supervisor's `AgentSpec::from_handle` used `root.to_path_buf()` for every cwd. Concurrent file mutations across agent sessions collided in a single working tree.

The v2 reframe (project-owner ratify msg 2158, pm ratify msg 2155, marketing relay msg 2161) positions teamctl as **the runtime that decomposes context across focused sessions** — *"each of them should work on a separate work tree, right?"*. Roles are context and attention boundaries, not expertise specialization. The marketing copy and the role-prompt templates promise five focused sessions; without per-session isolation, that promise is honest in prose but unenforced at runtime.

This is the prose-runtime drift class the existing substrate constraints exist to prevent. ADR 0004 (`.team/` folder + management UX) and substrate constraints #3 (byte-for-byte hand-authored) and #4 (every action reproducible by hand-editing YAML) all share the same shape: **what the prose promises, the substrate enforces**. Per-session isolation is the missing sibling.

## Decision

Make per-session git-worktree isolation a first-class teamctl runtime primitive, gated on explicit operator opt-in via `supervisor.worktree_isolation: true`. Each agent's tmux session then launches in its own worktree under `<root>/state/worktrees/<agent>/` on its own `agents/<agent-id>` branch.

Schema additions:

- Project-level: `supervisor.worktree_isolation: true | false`.
- Per-agent: `cwd_override: <path>` for advanced opt-out.

Resolution order for an agent's tmux `cwd`:

1. `agent.cwd_override` if set.
2. `<root>/state/worktrees/<agent-id>/` if `worktree_isolation` is **explicitly** `true`.
3. The project's resolved `cwd` (legacy single-cwd behaviour — matches pre-v2-A and the field-absent / explicit-`false` paths).

**Field-absent runtime semantics: legacy.** Existing pre-v2-A teams upgrade without their tmux sessions silently moving directories. `teamctl validate` emits a one-time warning nudging operators to set the field explicitly: `worktree_isolation: true` to opt in, or `worktree_isolation: false` to silence the warning while keeping legacy. Standard deprecate → warn → opt-in → next-major-flips cadence. New teams scaffolded by `teamctl init` ship `worktree_isolation: true` already wired in.

`teamctl up` provisions worktrees idempotently before tmux launch when the flag is on. `teamctl down` preserves them; `teamctl down --clean-worktrees` is the explicit destructive opt-in.

Coordination back to a shared branch piggybacks on the existing HITL `merge_to_main` approval flow — no new mechanism. Worker commits land on `agents/<worker>`; manager merges from inside its own worktree; the operator's actual default branch (`main`) only moves on operator-approved merges.

## Rationale

- **Substrate enforces what prose promises.** "Five focused sessions, each its own context" is now structurally true — concurrent agents cannot stomp on each other's working trees by construction. Same shape as ADR 0001 (SQLite WAL enforces concurrent-readers / single-writer) and the yaml_edit substrate (load-and-save preserves comments by construction).
- **Branch namespace `agents/` keeps the operator's branch space clean.** Operator's `main`, feature branches, work-in-progress checkouts are all untouched no matter what the agents do.
- **Idempotent up + preserve-by-default down.** Matches the wider "state preserved across `down && up`" promise. Operators don't lose worker work on routine restarts; they opt in to destructive cleanup.
- **HITL `merge_to_main` already exists** in the default sensitive-actions list (see `examples/oss-maintainer/.team/team-compose.yaml`). Coordination through this flow keeps v2-A from inventing a new mechanism for what the existing primitives already handle.
- **No auto-`git init`.** Initializing a repo is a real decision the operator should make explicitly. `teamctl validate` fails with a canonical message (`worktree_isolation requires a git repo at project.cwd. Run \`git init\` there or set \`supervisor.worktree_isolation: false\`.`) instead of silently shaping the user's filesystem.

## Alternatives considered

- **Auto-`git init` if `project.cwd` isn't a repo.** Rejected: silent magic. The operator should know when `git init` happened to their tree.
- **One worktree per project (managers + workers share).** Rejected: defeats the purpose. The collision class is between *any* two concurrent agents, not just across projects.
- **Worker-to-worker direct merges.** Out of v2-A scope. Worker collaboration uses dm/broadcast for prose; code-merge routes through the manager via HITL. Adding worker-peer merge would be a new approval shape; defer until a real workflow needs it.
- **Cross-runtime worktree semantics in v2-A.** Out of scope. Codex / Gemini sister plugins inherit the substrate via the teamctl runtime; their plugin-side onboarding teaches the property. v2-A is the substrate; sister plugins are downstream.

## Consequences

- **Schema migration:** existing pre-v2-A teams keep their legacy single-cwd behaviour at runtime — field-absent does NOT silently flip to per-session worktrees. Validate emits a one-time warning prompting explicit opt-in. The non-git-`project.cwd` hard-error fires only when `worktree_isolation: true` is set explicitly. New teams scaffolded by `teamctl init` ship the field set to `true`. Net effect: the substrate ships and is the going-forward default for new teams, while existing teams confirm their direction at the time of their next deliberate update — nothing about a running team moves under the operator's feet.
- **`teamctl up` startup time:** first `up` adds one `git worktree add` per agent. ~50-100ms each on warm caches. Subsequent ups are no-ops (existing worktrees reused).
- **Disk usage:** each worktree carries a working-tree-sized checkout. For a 5-agent team on a 100MB repo, that's ~500MB additional. Acceptable for teamctl's typical scale (small-to-mid repos, ≤10 agents).
- **Coordination latency:** every code merge to `main` is HITL-gated. This is by design — operators don't want their main branch moving without their say-so.
- **Sister plugins (T-077-v2-B onwards) inherit the property.** OpenCode, Codex CLI, and Gemini CLI plugins teach the same substrate without re-implementing it.

## Implementation notes

- `team-core::worktree` exposes the small primitives (`ensure_worktree`, `remove_worktree`, `is_git_repo`, `branch_for_agent`, `default_worktree_path`).
- `Compose::resolve_agent_cwd` is the single source of truth for the resolution rules.
- `AgentSpec::from_handle(h, &compose)` reads through `resolve_agent_cwd` so every supervisor caller gets the same answer.
- `cmd::up::ensure_agent_worktrees` orchestrates `git worktree add` before tmux launch; `cmd::down::clean_agent_worktrees` handles the `--clean-worktrees` flag.
- `validate::warnings` returns non-blocking advisories; `teamctl validate` and `teamctl up` both surface them. Existing `validate::validate` returns hard errors only.

## Unblocks

- T-077-v2-B — plugin reframe (rationales redraft + role-prompt-style.md Section 4 rewrite + Stage 2/3 prose).
- T-077-v2-C — version check.
- T-077-v2-D — project-aware suggestions.
- T-077-v2-E — custom-org first-class.

All four downstream tickets gate on this substrate landing on `main`.
