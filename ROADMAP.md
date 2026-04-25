# Roadmap

## v0.1 — shipped

- Declarative `team-compose.yaml` + validator
- Portable `tmux` supervisor (Linux + macOS)
- SQLite mailbox with MCP-stdio server: `dm`, `broadcast`, `inbox_*`, `list_team`, `org_chart`, `request_approval`
- Multi-runtime: Claude Code, Codex CLI, Gemini CLI
- Per-agent ACLs (`can_dm`, `can_broadcast`), channels, project isolation
- Inter-project manager bridges with TTL and audit log
- HITL permission fabric (default sensitive-action list, auto-approve windows)
- Rate-limit detection per runtime, configurable hook chain (wait / send / webhook / run)
- Telegram interface adapter with inline approval UI
- `teamctl validate / up / down / reload / status / logs / send / bridge / pending / approve / deny / budget / gc`
- `cargo-dist` release pipeline + install script

## Near-term

- `systemd --user` supervisor back-end for Linux hosts that want reboot-survivability.
- `launchd` supervisor back-end for macOS.
- Additional interface adapters: Discord, iMessage, CLI chat, webhook.
- Email interface (IMAP inbound + SMTP outbound) — configured declaratively, same shape as `telegram`.
- Runtime cost parsers for per-session USD tracking (Claude `/cost`, Codex per-msg, Gemini summary).
- Crash-loop detector + idle-deadlock watchdog.
- Self-hosted teamctl-dev team: teamctl develops teamctl (dogfooding).

## Longer term

- Nested sub-teams beyond manager → workers.
- Multi-host distribution.
- Nix flake.
- Web dashboard (read-only first) for operators.
