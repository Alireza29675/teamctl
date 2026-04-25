# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — 2026-04-25

### Added

- `team-core` — YAML schema, validator, renderer, `Supervisor` trait with portable `TmuxSupervisor`.
- `team-mcp` — stdio JSON-RPC MCP server with `whoami`, `dm`, `broadcast`, `inbox_peek/ack/watch`, `list_team`, `org_chart`, `request_approval`.
- `teamctl` CLI — `validate`, `up`, `down`, `reload`, `status`, `logs`, `send`, `bridge open/close/list/log`, `pending`, `approve`, `deny`, `budget`, `gc`.
- `team-bot` — Telegram interface adapter with inline approval UI and `--manager` scoping.
- Runtime adapters for Claude Code, Codex CLI, Gemini CLI.
- Project isolation; time-boxed inter-project manager bridges; HITL permission fabric with default sensitive-action list.
- Interfaces abstraction (Telegram, Discord, iMessage, CLI, webhook — Telegram adapter shipped; others documented).
- Astro Starlight docs site scaffold + Cloudflare Pages deploy workflow.
- `cargo-dist` release pipeline, `install.sh`, Homebrew tap config, crates.io publish.
- Examples: `hello-team`, `multi-agent`, `multi-runtime`, `two-projects`, `newsletter-office`, `startup-team`, `market-analysts`.
- 28 unit + integration tests.
