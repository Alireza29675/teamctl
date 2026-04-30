# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] — 2026-04-30

### Added

- Per-manager bot scoping for Telegram approval routing. Approval
  cards now reach exactly one chat — the bot scoped to the manager
  that the requesting agent reports to — instead of fanning out to
  every connected bot. Multi-hop manager chains walk up to two
  levels today; deeper chains are tracked as a follow-up.
- Approval delivery state on the broker. The `approvals` table
  grows a nullable `delivered_at REAL` column and a new terminal
  status `undeliverable`. When `expires_at` elapses, rows with
  `delivered_at IS NULL` end as `undeliverable`; rows that were
  surfaced to a human end as `expired` (existing behaviour).
  Callers can now distinguish "the human never saw the prompt"
  from "the human declined to respond."
- `wait: bool` argument on the `request_approval` MCP tool
  (default `true`). `wait: false` returns the freshly inserted
  row's status immediately, skipping the long-poll — useful for
  fire-and-forget callers and diagnostic tooling.
- Telegram approval cards now resolve in place. Tapping Approve
  or Reject edits the message to show the outcome and removes the
  buttons. Stale taps on a duplicate copy answer with
  `#<id> already resolved` and leave the row untouched.
- Plain-text rendering for outbound Telegram messages. Markdown
  syntax (`**bold**`, `_italic_`, `- bullets`) is stripped before
  send so chat surfaces don't render literal punctuation. Buttons
  (approval cards) are unaffected.
- Context-override warning on read-side commands. `teamctl ps`,
  `mail`, and `inspect` now print a stderr note when active
  context or `TEAMCTL_ROOT` overrides walk-up resolution, with the
  source of the override called out (CLI flag vs environment).
- `oss-maintainer` example. Pipeline workflow + cross-channel ACLs
  + plan-mode HITL on release-critical actions. Demonstrates a
  triage / bug-fix / docs / release-manager team for an open-source
  maintainer.
- `indie-game-studio` example. Plan-mode dissenter on a creative
  team + private critique channel. Demonstrates a director /
  designer / writer / playtest-critic team where the critic vetoes
  privately rather than publicly.
- Cookbook section under `docs/cookbook/`. Captures patterns from
  examples that are too narrow to ship as their own example folder
  (multi-agent ACL composition, multi-runtime cohabitation,
  cross-project bridges).
- Lychee link-checker on the docs CI. Internal link breakage fails
  PRs that touch `docs/`; external links warn-only to keep the
  check stable against third-party HTTP flakiness.

### Changed

- Author voice across source code, doc-comments, operator-references,
  example fixtures, and landing copy is now project-voice — the
  project speaks as itself rather than through a personal first-person
  maker. Author attribution metadata (LICENSE copyright, Cargo
  authors, ADR `Author:` lines) is preserved as factual.
- Cookbook prose for the `oss-maintainer` example softened to match
  what the example actually demonstrates (single-project) rather
  than the cross-project framing that lived in earlier drafts.
- Docs deploy workflow's deploy step now runs on both `push` to
  `main` and `workflow_dispatch`, so manual redeploys via
  `gh workflow run docs.yml` actually deploy.

### Removed

- Deprecated example folders: `multi-agent`, `multi-runtime`,
  `two-projects`. The patterns they demonstrated (channels + ACL
  composition, multi-runtime cohabitation, project bridges) survive
  in `startup-team`, `newsletter-office`, `oss-maintainer`,
  `indie-game-studio`, and the new cookbook recipes.
- `WhyIBuiltThis.astro` landing-page section. Was a placeholder
  waiting on a personal-voice interview that the project-voice shift
  retired.

## [0.2.9] — 2026-04-26

### Added

- `reply_to_user` MCP tool. Managers (`is_manager: true`) can now talk
  back to the human operator who DMed them; the configured interface
  adapter (Telegram, Discord, ...) forwards the reply. Inserts a
  message row with `recipient = "user:telegram"`. Workers calling it
  get an explicit error -- inter-agent traffic stays on `dm`.
  Companion: `Store::is_manager(agent_id)` lookup against the
  `agents` table.
- Telegram bot bootstrap UX. A `/start` from a chat that isn't on the
  allow list now replies with the chat's numeric id and a copy-paste
  hint for `.env`, removing the @userinfobot detour during first-run
  setup. `TEAMCTL_TELEGRAM_CHATS` accepts an empty value to make
  bootstrap reachable.

### Changed

- Telegram bot's outbound stream now forwards messages whose
  `recipient = 'user:telegram'` (the `reply_to_user` output) and
  ack's them via `acked_at`. Previously it forwarded messages going
  *into* managers, which surfaced inbound traffic instead of
  outbound replies.
- `.gitignore`: added `.env` and `**/.env` so Telegram tokens and
  per-team secrets don't get committed.

## [0.2.8] — 2026-04-26

### Fixed

- aarch64-unknown-linux-gnu Release builds, take 4. With the cross-gcc
  installed (v0.2.7), the C parts compiled but the **Rust linker** still
  defaulted to the host's x86_64 `rust-lld`, producing "is incompatible
  with elf64-x86-64" on every aarch64 object. Added `.cargo/config.toml`
  with `target.aarch64-unknown-linux-gnu.linker = "aarch64-linux-gnu-gcc"`
  so cargo invokes the cross linker for that target.

## [0.2.7] — 2026-04-26

### Fixed

- aarch64-unknown-linux-gnu Release builds (final). Even with rustls
  in v0.2.6, `ring` (rustls's crypto provider) needs to compile its
  ARM assembly using `aarch64-linux-gnu-gcc`, which the GitHub Actions
  ubuntu-24.04 runner doesn't ship by default. Configured cargo-dist's
  `[workspace.metadata.dist.dependencies.apt]` to install
  `gcc-aarch64-linux-gnu` only on the aarch64-linux build matrix
  entry, so cc-rs auto-resolves the cross compiler.

## [0.2.6] — 2026-04-26

### Changed

- `team-bot` now uses **rustls** instead of native-tls. Vendoring
  OpenSSL in v0.2.5 wasn't enough -- building openssl-src from source
  also needs `aarch64-linux-gnu-gcc`, which isn't on the GitHub Actions
  cross-build runner. rustls is pure Rust with zero C dependencies, so
  it cross-compiles cleanly to every dist target. Switched
  teloxide's features to `default-features = false` +
  `["macros", "ctrlc_handler", "rustls"]`.

## [0.2.5] — 2026-04-26

### Fixed

- (intended) aarch64-unknown-linux-gnu Release builds via vendored
  OpenSSL. Released to crates.io but the build still failed because
  the openssl-src vendored build still requires
  `aarch64-linux-gnu-gcc` which isn't installed on the runner.
  Superseded by 0.2.6's switch to rustls.

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
