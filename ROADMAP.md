# Roadmap

## 0.1.0 — hello-team → multi-project

Shipped in phases, each a separate PR with its own example and docs page.

- [x] Phase 0 — repo scaffold, workspace compiles, CI green
- [ ] Phase 1 — `hello-team`: one manager, Claude Code, SQLite mailbox, `teamctl up/down/reload/status/logs`
- [ ] Phase 2 — channels + ACLs: `broadcast`, `can_dm`, `can_broadcast`
- [ ] Phase 3 — multi-runtime: Codex CLI + Gemini CLI adapters
- [ ] Phase 4 — multi-project + bridges
- [ ] Phase 5 — HITL permission fabric (`request_approval`, auto-approve windows)
- [ ] Phase 6 — Telegram bot (`team-bot`)
- [ ] Phase 7 — budget, crash-loop / deadlock watchdogs, message TTL
- [ ] Phase 8 — Starlight docs site on `teamctl.run`
- [ ] Phase 9 — release engineering: `cargo-dist`, Homebrew tap, install script

## 0.2.0

- macOS support (`launchd` back-end)
- Hot reload of channel ACLs
- Per-runtime cost normalization
- Self-hosted teamctl-dev team ("dogfooding")

## Longer term

- Nested sub-teams beyond manager → workers
- Multi-host distribution
- Nix flake
