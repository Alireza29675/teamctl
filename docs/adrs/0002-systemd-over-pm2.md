# ADR 0002 — systemd template units over PM2

- Status: accepted
- Date: 2026-04-24

## Context

teamctl must keep N long-lived agent processes alive 24/7. The target host already runs PM2 (for Cloudflare tunnels) and `systemd --user` units (for schedx).

## Decision

Use **`systemd --user` template units** (`agent@<project>-<agent>.service`).

## Rationale

- Template units are purpose-built for "N copies of the same thing," which is exactly our shape.
- Already in the target stack — no new supervisor to install.
- `Restart=always`, `RestartSec=5`, per-unit journal namespace, and `systemctl --user status` give us lifecycle + observability for free.
- PM2 adds an 83 MB Node daemon and forces us to keep a separate `ecosystem.config.js` in sync with the compose file.

## Consequences

- macOS support requires a `launchd` back-end (see 0.2 roadmap). We model the supervisor as a `Supervisor` trait in `team-core` so this is additive.
- Unit files live in `state/` (generated). Users never edit them by hand — if they do, `teamctl reload` overwrites.
