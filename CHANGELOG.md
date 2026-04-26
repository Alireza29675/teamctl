# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `teamctl rl-watch` now spawns the runtime under a real pseudo-terminal
  (via `portable-pty`) and forwards stdin from the wrapper's controlling
  TTY. Without this, runtimes detected non-TTY stdio and silently dropped
  into one-shot/print mode -- so `tmux attach -t a-<agent>` showed a
  five-second restart loop instead of an interactive Claude Code REPL.
  Rate-limit pattern scanning is preserved by tee-ing the pty's output
  through an ANSI-stripping line scanner before re-emitting it.
- `agent-wrapper.sh` now passes runtime arguments as proper `argv` to
  `teamctl rl-watch -- "$BIN" "$@"` instead of round-tripping them
  through a single `$BIN_ARGS` string. The old shape silently word-split
  multi-word values like `--append-system-prompt "$(cat role.md)"`,
  feeding the runtime garbage. The wrapper also appends a configurable
  `BOOTSTRAP_PROMPT` (defaults to "Begin your shift as <agent>. Open
  inbox_watch via team MCP. Stay running.") so agents enter their work
  loop on launch instead of sitting at an empty prompt.
- `teamctl up` rewrites `bin/agent-wrapper.sh` whenever the on-disk copy
  differs from the binary's bundled template. Previously the wrapper was
  written only on first launch, so upgrading teamctl never delivered
  wrapper fixes to existing workspaces.
- `teamctl up` auto-accepts Claude Code's per-workspace trust dialog for
  every cwd that will host a `claude-code` agent (writes
  `hasTrustDialogAccepted: true` into `~/.claude.json`). Running `teamctl
  up` is itself an explicit "I trust this directory" signal -- without
  this, the runtime blocks on a trust prompt the moment it boots and
  defeats the "agents start working when teamctl up runs" model.

### Fixed

- Runtime adapter descriptors for the three shipped runtimes (Claude Code,
  Codex, Gemini) are now embedded in the `team-core` binary instead of
  being read from a `runtimes/` directory at the compose root. Without
  this, every fresh install (`teamctl init` + `teamctl up`, or any
  `cargo install` / Homebrew / `install.sh` flow) tight-looped with
  `runtime 'claude-code' for agent 'X' has no descriptor in runtimes/`
  because the YAMLs only existed inside the source tree and were never
  packaged. `<root>/runtimes/<id>.yaml` continues to work as an override,
  matching the design intent in ADR 0004 ("optional overrides for shipped
  runtimes"). Validator and `rl-watch` error messages now reflect that
  the missing-runtime case means no built-in *and* no override.

## [0.1.2] — 2026-04-25

### Fixed

- Release pipeline now produces GitHub Release artifacts. v0.1.1 published
  to crates.io but the hand-written cargo-dist workflow ran cross-compile
  on a single Ubuntu runner, so no platform tarballs were ever uploaded.
  Regenerated `release.yml` from `dist generate` (proper job matrix) and
  split crates.io publishing into a sibling `publish-crates.yml`.

## [0.1.1] — 2026-04-25

### Added

- Rate-limit handling. Every runtime invocation flows through
  `teamctl rl-watch`, which detects rate-limit signatures from the
  runtime's `rate_limit_patterns`, records them in a new `rate_limits`
  table, runs a configurable hook chain (`wait` / `send` / `webhook` /
  `run`), and waits until the limit clears before letting the wrapper
  respawn — replacing the previous 5-second tight retry.
- Per-agent `on_rate_limit:` override and a global `rate_limits.hooks:`
  block with `default_on_hit` chain.
- Runtime descriptor field: `rate_limit_patterns` with optional
  `resets_at_capture` / `resets_in_capture` regexes.
- Docs: `docs/concepts/rate-limits.md`.

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
