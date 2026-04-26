# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.4] — 2026-04-26

### Fixed

- Release builds for every platform. cargo-dist 0.25.1's default runner
  labels (`ubuntu-20.04`, `macos-13`) were both retired by GitHub
  Actions in 2025-2026 -- jobs targeting them sit queued forever.
  Override every target via inline
  `github-custom-runners = { x86_64-unknown-linux-gnu = "ubuntu-24.04",
   aarch64-unknown-linux-gnu = "ubuntu-24.04",
   x86_64-apple-darwin = "macos-14",
   aarch64-apple-darwin = "macos-14" }`.
  v0.2.3 attempted this with the `[workspace.metadata.dist.github-custom-runners]`
  table syntax; cargo-dist 0.25.1's deserializer rejects that with
  "invalid type: sequence, expected a string" -- the inline-table form
  is what the v0 schema actually accepts.

## [0.2.3] — 2026-04-26

### Fixed

- (intended) macOS Release builds via `github-custom-runners` table.
  Released to crates.io but the Release workflow rejected the table
  syntax. Superseded by 0.2.4's inline form.

## [0.2.2] — 2026-04-26

### Fixed

- Release pipeline. v0.2.0 and v0.2.1 published to crates.io but
  produced no GitHub Release artifacts (no platform tarballs, no
  Homebrew formula bump) because `dist host` exited 255 on a freshness
  check: the hand-edited `runs-on: ubuntu-24.04` in `release.yml`
  diverges from what `cargo-dist 0.25.1` would generate
  (`ubuntu-20.04`, retired by GitHub Actions in April 2026). Adding
  `allow-dirty = ["ci"]` to the dist metadata tells dist to skip the
  workflow-freshness diff so releases unblock.
- Docs build (Astro Starlight). The Astro 4.16 / Starlight 0.29 pin
  pulled in newer transitive `zod` versions whose internal v4 API
  layout broke `zod-to-json-schema`. Bumped to Astro 5 + Starlight
  0.30, both of which handle modern zod cleanly.

## [0.2.1] — 2026-04-26

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
- `claude-code` agents now launch with `--dangerously-skip-permissions`
  in addition to whatever `permission_mode:` the agent sets. Auto mode
  in Claude Code still prompts for tool calls its risk classifier deems
  sensitive (anything matching `claude mcp *`, `git push`, ...). With
  no human at the keyboard those prompts deadlock the pane, so the
  classifier becomes advisory and the prompt is suppressed. The proper
  human-in-loop ring for teamctl is the team-mcp `request_approval`
  tool gated by the agent's `autonomy:` field -- not the per-tool-call
  prompt buried inside the runtime.

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
