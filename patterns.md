# Patterns

Coding and repo patterns this project follows. Accretes as the team learns; review when refactoring so the patterns stay current with the code. Each pattern: short name, the rule, one example or pointer.

## Per-ticket folders under `.team/tasks/`

Every ticket gets its own folder at `.team/tasks/[YYYY-MM-DD]-[slug]/` containing `TASK.md` (goal + acceptance) and, for substantive work, sibling `SPEC.md`, `DESIGN.md`, or `PHASE-N.md` files. Branch names mirror the ticket id (`T-NNN/short-slug`); worktrees mirror the slug under `.worktrees/T-NNN-<slug>/`. Example: `.team/tasks/2026-05-03-teamctl-cc-plugin/`.

## Push delivery, not polling, on the MCP bus

`team-mcp` emits `notifications/claude/channel` events when a new message lands; agents subscribe via `inbox_watch` and react inline rather than spinning a poll loop. Same shape for the Telegram bridge: HITL approvals surface as inline keyboard prompts, not periodic refreshes. The pattern keeps idle CPU at zero and latency tied to the broker's notify, not a polling interval.

## Worktree per ticket, never edit another agent's tree

Each in-flight ticket gets its own worktree under `.worktrees/` at the repo root. Agents never edit files in another agent's worktree — cross-tree work goes through review on the source branch instead. The main worktree stays on its primary branch and isn't repurposed for unrelated work.
