---
title: ADR 0002 — Portable tmux supervisor, pluggable `systemd` / `launchd` back-ends
---

- Status: accepted (supersedes earlier draft that locked systemd as the only back-end)
- Date: 2026-04-24

## Context

teamctl must keep N long-lived agent processes alive. Original plan: `systemd --user` template units, Linux-only. macOS has no systemd, and we wanted first-class dev on macOS, so supervision had to abstract.

## Decision

Model supervision as a trait in `team-core`:

```rust
pub trait Supervisor {
    fn up(&self, agents: &[AgentSpec]) -> Result<()>;
    fn down(&self, agents: &[AgentSpec]) -> Result<()>;
    fn restart(&self, agent: &AgentSpec) -> Result<()>;
    fn status(&self, agent: &AgentSpec) -> Result<AgentStatus>;
}
```

v0.1 ships one implementation: **`TmuxSupervisor`** — for each agent, spawns a detached `tmux new-session -d -s a-<project>-<agent>` running the agent wrapper. The wrapper is a simple `while true; do …; sleep 5; done` loop, so crashes restart within 5 s without needing a system supervisor.

Two production back-ends plug in behind the same trait:

- `SystemdSupervisor` — `~/.config/systemd/user/agent@.service` template unit, `Restart=always`, survives reboot.
- `LaunchdSupervisor` — `~/Library/LaunchAgents/run.teamctl.<project>.<agent>.plist`, `KeepAlive=true`, survives reboot.

`broker.supervisor.type` in `team-compose.yaml` selects the back-end (`tmux` | `systemd` | `launchd`). Default is `tmux`.

## Rationale

- macOS is a first-class development surface; requiring `systemd` from day one would force the author to SSH to a Linux box for every iteration.
- The wrapper's in-process restart loop gives us crash-recovery within seconds for free — that covers 90 % of "why you want a supervisor" during development.
- Users who want reboot-survivability on production hosts opt into `systemd` or `launchd` in one line.
- Keeps the supervisor choice reversible — we can reshape or add (`s6-overlay`, `pm2`) without touching agent logic.

## Consequences

- `TmuxSupervisor` alone does **not** survive machine reboot. Documented clearly in [operating-in-production](../guides/operating-in-production.md).
- `teamctl status` queries are back-end specific; the trait's `status()` normalizes them.
- Integration tests have two shapes: a "tmux" lane runnable on macOS and Linux CI, and a "systemd" lane that runs only on Linux with `--privileged` or on a real host.
