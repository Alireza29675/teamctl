# Roadmap

> Last updated: 2026-04-28 (against v0.2.9). The full release-by-release
> history lives in [`CHANGELOG.md`](./CHANGELOG.md); this file is the
> short answer to "what's done, what's next, what's later".

## Shipped (through v0.2.9)

### Core platform

- Declarative `team-compose.yaml` + projects/`<id>.yaml` schema, validator that collects all errors before reporting, and artifact renderer.
- Portable `tmux` supervisor (Linux + macOS) with auto-respawn.
- SQLite mailbox with stdio MCP server: `dm`, `broadcast`, `inbox_peek` / `inbox_ack` / `inbox_watch`, `list_team`, `org_chart`, `reply_to_user`, `request_approval`.
- MCP `notifications/claude/channel` events — Claude Code agents wake on new mail without polling.
- Per-agent ACLs (`can_dm`, `can_broadcast`), channels, project isolation.
- Inter-project manager bridges with TTL and audit log.
- HITL permission fabric: default sensitive-action list, auto-approve windows, blocking `request_approval` on tagged tool calls.
- Multi-runtime: Claude Code, Codex CLI, Gemini CLI. Runtime descriptors are embedded in the `team-core` binary so a fresh install works without copying YAMLs.
- Per-runtime rate-limit detection with a configurable hook chain (`wait` / `send` / `webhook` / `run`), replacing the original 5-second tight retry.
- `teamctl rl-watch` runs the runtime under a real pseudo-terminal, so Claude Code / Codex / Gemini see a TTY and stay interactive.
- Telegram interface adapter (teloxide + rustls) with inline approval UI, per-manager scoping, per-bot ID namespacing, and a `/start` bootstrap that echoes the chat id for `.env`.

### CLI surface

- Lifecycle: `init`, `validate`, `up`, `down`, `reload`.
- Inspection: `ps`, `logs`, `tail`, `mail`, `inspect`.
- Mailbox: `send`.
- Approvals: `approvals`, `approve`, `deny`.
- Bridges: `bridge open` / `close` / `list` / `log`.
- Housekeeping: `budget`, `gc`, `rl-watch`.
- Attach / exec: `attach` (read-only by default, `--rw` confirms), `exec`, `shell`.
- Workspace UX: `env` / `env --doctor`, `context` (switch between named `.team/` roots), `bot setup` / `list` / `status` (per-manager Telegram bot wizard, ADR 0005), `update` (self-update by re-running shell installer / brew / cargo).

### Release & docs

- `cargo-dist` 0.25.1 release pipeline, four targets (Linux x86_64 / aarch64, macOS x86_64 / aarch64), curl-pipe `install.sh`.
- Crates published to crates.io via a sibling `publish-crates.yml` workflow.
- Astro Starlight docs site under `docs/`, deployed to Cloudflare Pages.
- Examples: `hello-team`, `multi-agent`, `multi-runtime`, `two-projects`, `newsletter-office`, `startup-team`, `market-analysts`.

## Near-term

- **Homebrew tap.** `Alireza29675/homebrew-teamctl` provisioned, `HOMEBREW_TAP_TOKEN` wired, cargo-dist's `publish-homebrew-formula` job re-enabled. First green run lands the brew install line back in the README.
- **`systemd --user` supervisor backend** for Linux hosts that want reboot-survivability without a tmux session.
- **`launchd` supervisor backend** for macOS, same shape.
- **Per-agent cost tracking.** Surface token / USD spend per session through `teamctl budget` (Claude `/cost`, Codex per-message, Gemini summary).
- **Crash-loop and idle-deadlock watchdogs.** A respawn pattern that's clearly not converging should page the operator, not retry forever.
- **More interface adapters.** Discord, iMessage, CLI chat, webhook, and email (IMAP in / SMTP out) — same `interfaces:` shape as `telegram`.
- **Self-hosted dogfooding.** A `teamctl-dev` team that develops teamctl using teamctl.

## Longer term

- Nested sub-teams beyond manager → workers (sub-managers reporting to managers).
- Multi-host distribution (workers on a different machine from their manager).
- Nix flake.
- Read-only web dashboard for operators.
