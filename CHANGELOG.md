# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Plugin slash commands renamed: `/teamctl:teamctl-init` → `/teamctl:init`,
  `/teamctl:teamctl` → `/teamctl:adjust`. Operators upgrading must use the
  new invocation forms.

## [0.7.0] — 2026-05-03

0.7.0 ships the Claude Code plugin. Install once (`claude plugin marketplace add https://github.com/Alireza29675/teamctl && claude plugin install teamctl@teamctl`), invoke `/teamctl-init`, and you're walked from no-teamctl-installed through a running supervised team in tmux in a few minutes. The plugin is teamctl's onboarding from inside Claude Code; the `.team/` directory it produces is the same hand-authorable YAML you've always had, byte-for-byte indistinguishable from one you'd type yourself. Parallel plugins for OpenCode, Codex CLI, and Gemini CLI are tracked at #59, #60, #61.

### Added

- **Claude Code plugin** at `plugins/claude-code/` (T-077). Two slash-invokable
  commands ship: `/teamctl-init` walks an operator from no-teamctl-installed
  to a running supervised team in tmux through a 7-stage flow (detect+install
  → pick a team shape from four named defaults → propose a named ASCII org
  tree → scaffold `.team/` to match `examples/<chosen>/.team/` byte-for-byte
  with role prompts generated against an 8-section spine → reveal beat
  → `teamctl up` → defer to `teamctl bot setup` for Telegram + voice-customize
  per manager → hand the keys back with the three lifecycle commands).
  `/teamctl` is the open-ended ongoing skill the operator keeps invoking
  afterwards: five v1 verbs (add manager, add worker, scope channel, wire
  telegram, retire agent) each running a Read → Propose → Confirm → Apply
  → Validate → Offer-reload loop with unified-diff receipts and substrate
  constraint #4 enforced (every action reproducible by `vim
  .team/team-compose.yaml`). Repo-root `.claude-plugin/marketplace.json`
  registers teamctl as a single-plugin marketplace; install via
  `claude plugin marketplace add https://github.com/Alireza29675/teamctl
  && claude plugin install teamctl@teamctl`.
- **Comment-preserving YAML edit substrate** at `team-core::yaml_edit`
  (T-077-E-prereq). Wraps `yaml-edit` with a bounded line-anchored helper
  for nested-block insertion. `teamctl bot setup`'s `interfaces.telegram`
  upsert path now routes through the substrate, preserving comments,
  blank-line clusters, and key ordering across edits — closing the
  recurring `.team/projects/<id>.yaml` round-trip regression class
  observed across 0.5.x and 0.6.x cascades.
- **`examples/solo-triage/`** as the fourth named-default team folder
  (T-077-B-prereq). Manager + research worker + inbox/journal worker;
  HITL on `publish` and `external_email`. Mirrors `oss-maintainer/`'s
  shape; serves as the byte-for-byte diff target for the plugin's
  scaffolding when the operator picks "Solo triage."
- **Repo-root `CLAUDE.md`** (T-077-F) carrying the cross-cutting rule
  that every release or substantive change to teamctl must consider
  impact on the plugin, the TUI, the docs, and the tests. Plus the
  4-bullet behavioural-guidelines spine (think before coding,
  simplicity first, surgical changes, goal-driven execution).
- **Three sister-plugin GitHub issues** for OpenCode CLI (#59), Codex
  CLI (#60), and Gemini CLI (#61). Each carries the spine sentence,
  the four substrate constraints, and links to the marketing
  positioning thread. External contributors can pick them up against
  the canonical Claude Code plugin reference.

### Changed

- **Examples env-var naming aligned to the canonical
  `TEAMCTL_TG_<NAME>_TOKEN/CHATS` pattern** (T-077-C-prereq). All five
  example folders' `.env.example` and `README.md` files now match the
  YAML-side env-var references — closing a drift class where copying
  `.env.example` literally would have set env vars the YAML didn't
  read. `startup-team` and `market-analysts` also gained yaml-canonical
  alignments (`PRODUCT_BOT_*` → `TEAMCTL_TG_PRODUCT_MANAGER_*`;
  `MARKETS_*` → `TEAMCTL_TG_CHIEF_*`).

### Notes

- **Tagged history note:** 0.5.2, 0.6.2, 0.6.3, and 0.6.4 were released
  on `main` as version-bumped `Cargo.toml` + CHANGELOG entries but were
  not tagged on origin (cargo-dist publish was not triggered for those
  bumps). 0.7.0 is the next tagged release after `v0.6.1`, superseding
  the 0.6.x untagged series.

## [0.6.4] — 2026-05-03

### Fixed

- **`reply_to_user` fanned out to every Telegram bot in the project.**
  `team-bot`'s outbound loop only filtered reply rows by `project_id`,
  so when a project ran one bot per manager (e.g. `pm`, `eng_lead`,
  `marketing` all in `sooleh`), every bot forwarded every reply and
  the operator received the same message three times under three bot
  avatars. The forward loop now applies `should_route` per row —
  mirroring the approvals path — so only the manager-scoped bot whose
  chain `manager_of(sender)` matches actually surfaces the reply.
  Unscoped bots keep the back-compat fallback (forward everything).

### Changed

- **Reply attribution moved to the end of the message.** Forwarded
  replies used to lead with `[sender] body`, which buried the actual
  content behind a tag the reader already knew (the bot avatar
  identifies the manager). Now the body comes first and the sender
  is appended as `\n\n— replied by <sender>`, so the message reads
  naturally and the attribution is a footer.
- **Expanded `reply_to_user` MCP tool description.** The tool now
  spells out for the model that it is the only channel back to the
  human (stdout never reaches the operator), that proactive replies
  are welcome, and that long-running work should ack first then
  reply on completion. The `text` field documents that delivery is
  plain text — no markdown, no headings, no code fences — and
  recommends sparing emoji use for scanability. `thread_id` now has
  a description (group the reply with the inbound channel meta's
  `thread_id`; omit for a fresh thread).

## [0.6.3] — 2026-05-03

### Fixed

- **Claude Code Channels never fired in-session.** `team-mcp`'s
  `initialize` response advertised only the `tools` capability, so
  Claude Code did not register a `notifications/claude/channel`
  listener and silently dropped every event the notifier emitted —
  mailbox rows accumulated without surfacing as `<channel
  source="team">` events. Initialize now declares
  `experimental.claude/channel: {}` (the documented capability that
  registers the listener), ships a recommended `instructions` string,
  and renames `serverInfo.name` from `team-mcp` to `team` so the
  rendered tag matches the `.mcp.json` key and the bootstrap prompt.
- **Channel notifications were dropped as wire-format violations.**
  `params.meta` is `Record<string, string>` per the Channels reference,
  but the notifier emitted `id` / `sent_at` as numbers and `thread_id`
  as `null` when unset. Claude Code dropped the malformed events
  silently, so even with the listener registered the agent never saw
  a `<channel>` tag — it was reaching the message only through the
  old `inbox_watch` long-poll. All meta values are now strings, and
  `thread_id` is omitted when not set.
- **Agent wrapper used `--channels` for an off-allowlist server.**
  Custom channels are silently dropped by `--channels` during the
  research preview. Wrapper now uses
  `--dangerously-load-development-channels server:team --` (with the
  `--` separator so the variadic flag does not swallow the bootstrap
  prompt).
- **Dev-channels confirmation dialog stranded agents on every
  restart.** Claude Code prompts "I am using this for local
  development" each time it boots with a non-allowlisted dev channel,
  with no persistent acceptance. Wrapper now side-spawns a watcher
  that polls its own tmux pane for the dialog header and presses
  Enter once, then exits (60 s deadline; no-op once team-mcp is
  allowlisted or when running outside tmux).

## [0.6.2] — 2026-05-02

### Fixed

- **`teamctl up` failed when `project.cwd` was a relative path.** The
  rendered per-agent env file omitted `TEAMCTL_ROOT`, so the wrapper
  fell back to `CLAUDE_PROJECT_DIR` (often a literal `..`). After the
  wrapper's `cd "$CLAUDE_PROJECT_DIR"`, the subsequent
  `teamctl --root ".." rl-watch …` resolved one directory above the
  intended `.team/`, and the runtime crash-looped with
  `read …/team-compose.yaml: No such file or directory`. Renderer now
  emits an absolute `TEAMCTL_ROOT=<compose.root>` so `--root` is
  pinned regardless of post-`cd` cwd.
- **Agent-wrapper crashed under `set -u` for agents without an
  `effort:` field.** The renderer only emits `EFFORT=` for agents
  that set it, but the wrapper unconditionally referenced `$EFFORT`
  via `[ -n "$EFFORT" ]`. With `set -u` active, that aborted the
  wrapper before exec — visible only after the `TEAMCTL_ROOT` fix
  let the wrapper progress past compose loading. Wrapper now
  defaults `EFFORT` to empty alongside the other optional vars.

## [0.6.1] — 2026-05-02

### Added

- **`teamctl update` — self-update command.** Detects the install
  method from `current_exe()`'s path (Cellar/teamctl → Homebrew,
  `~/.cargo/bin/` → cargo, otherwise the shell installer) and re-runs
  the matching update flow. Checks GitHub Releases for the latest
  version first; no-ops when already current. Flags: `--check` (just
  print the version comparison), `--yes` (skip confirmation),
  `--method <shell|brew|cargo>` (override autodetect). New guide at
  `/guides/updating/`. Closes the gap that caused v0.5.2 and v0.6.0
  to ship late — once update is in the wild, operators can pull each
  release without remembering the curl-pipe by hand.

## [0.6.0] — 2026-05-02

### Added

- **`teamctl bot setup` — interactive 1:1 Telegram bot wizard.**
  Walks BotFather → token → `/start` → chat id for every manager,
  prompts for env-var names with sensible defaults, writes
  `.team/.env` (idempotent upsert; existing vars preserved), and adds
  an `interfaces.telegram` block to that manager in
  `projects/<id>.yaml`. **Resumable**: fully-configured managers
  skip silently, partials only re-ask for the missing piece (token or
  chat id), and YAML-fixed env-var names are reused without
  re-prompting. Positional `[manager]` arg scopes the wizard
  (`teamctl bot setup news:head_editor`); `--force` re-asks for
  everything. Sibling `bot list` shows env-var status; `bot status`
  shows running tmux sessions. ADR 0005.
- **Per-manager Telegram bots auto-spawn under `teamctl up`.** One
  `team-bot` tmux session per manager-with-`interfaces.telegram`,
  named `<prefix>bot-<project>-<role>`, scoped via `--manager` so
  each bot only sees its manager's traffic. `teamctl down` stops
  them alongside agents. Skips with a warning when the token env var
  is unset (no hard fail — agents still come up).
- **DM-the-bot routing in `team-bot`.** Plain text on a manager-scoped
  bot is now treated as a message to that manager; no `/dm role text`
  ceremony required. The `/start` and `/help` replies on a scoped
  bot tell the operator which manager they're talking to. `/dm`,
  `/pending`, and inline approval buttons remain as escape hatches.

### Changed

- **Telegram config moved from top-level `interfaces:` to per-manager
  `interfaces.telegram`.** The new shape lives directly on the
  manager definition in `projects/<id>.yaml`, keeping related fields
  together and removing a YAML cross-reference. The top-level
  `interfaces:` array is reserved for non-Telegram adapters
  (Discord, iMessage, CLI, webhook) — those still fit the
  array-of-named-channels shape better.
- **`telegram_inbox: true` is removed.** Presence of
  `interfaces.telegram` on a manager is the new "this manager
  receives Telegram forwards" signal. Validation now flags an
  `interfaces.telegram` block on a worker the same way the old
  `telegram_inbox: true` flag did.
- **`reports_to_user: true` is removed.** The flag was already
  functionally inert — `reply_to_user` gates on `is_manager`, not
  this — and overlapped semantically with `interfaces.telegram`.
  Dropping it is a strict simplification: one fewer field in the
  schema, the docs, the templates, and every example. Old YAMLs
  carrying the line still parse (the field is silently ignored, no
  hard break).
- Examples (`startup-team`, `oss-maintainer`, `indie-game-studio`,
  `market-analysts`, `hello-team`) and the dogfood `.team/` migrated
  to the new shape; their `.env.example` entries align with the
  `TEAMCTL_TG_<MANAGER>_TOKEN` / `_CHATS` defaults the wizard picks.

### Migration

- If you wired Telegram by hand via the old top-level `interfaces:
  - type: telegram` block, `team-bot` keeps running against
  whatever you start manually. To switch to auto-spawn, run
  `teamctl bot setup` (it will skip managers whose env vars are
  already populated unless you pass `--force`) and remove the
  legacy top-level entry.
- If you had `telegram_inbox: true` or `reports_to_user: true` on
  any agent, drop the lines — neither is in the new schema. They're
  silently ignored on existing YAML, but cleaning them up is the
  intended end state. The validator will tell you if any worker
  accidentally inherits an `interfaces.telegram` block.

## [0.5.2] — 2026-05-02

### Added

- **`team-mcp` pushes new mail as Claude Code Channels notifications.**
  When the connected client is Claude Code v2.1.80+ launched with
  `--channels server:team`, `team-mcp` emits
  `notifications/claude/channel` for every new inbox row addressed
  to the agent. The runtime injects each event as a
  `<channel source="team">` tag, so agents react on arrival without
  polling and idle silently between events. The wrapper sets the
  flag automatically for the claude-code runtime; bootstrap prompt
  rewritten to expect channel events and use `inbox_peek` for
  restart catch-up only. Codex/Gemini paths unchanged. README and
  ROADMAP have promised this since v0.2.9 — first release that
  actually ships it.

## [0.5.1] — 2026-05-02

### Fixed

- **`teamctl ui` approve modal accepts lowercase `y`.** Previous
  uppercase-only matcher meant `y` did nothing — operators concluded
  the modal was broken. Asymmetric chord shape now: `y` or `Y`
  approve (loose, common path); `N` only deny (strict, preserves
  destructive-deny Shift-gate). Modal label and help overlay both
  reflect the new shape.
- **Tutorial body wraps to modal width.** Long step descriptions
  no longer extend past the modal — `Wrap { trim: true }` on the
  Paragraph render.
- **Tab cycles pane focus uniformly.** Previously Tab cycled INTO
  mailbox tabs (Inbox → Channel → Wire) instead of moving to the
  next pane — operators got stuck. Tab now consistently cycles
  Roster → Detail → Mailbox → Roster across all panes. New `[`
  and `]` chords walk mailbox tabs when Mailbox is focused (vim
  `[t`/`]t` mental model).
- **Statusline pins Tab pane-cycle hint always-visible.** First
  segment of every statusline now reads "Tab cycle panes" so the
  chord is discoverable from the very first launch. Mailbox-focused
  contextual hint updated to "[ / ] tabs."
- **Tmux ANSI colors render in detail pane.** Captured agent output
  now passes through `tmux capture-pane -e` and parses through
  `ansi-to-tui` (MIT, MSRV 1.78). Falls back to raw text on parse
  error so malformed escapes don't crash the render.

### Notes

- Release-pipeline gap caught alongside this patch — cargo-dist
  smoke-test case-statement order matters when binary-name prefixes
  overlap (`teamctl-ui` vs `teamctl`). Always put longest-prefix
  branches first. Same bug shape as the splash isometric4 figlet
  glyph collision.
- TUI bug cluster (1) detail-pane height + (2) mailbox-bottom-half
  layout (operator preference) tracked as T-074 PR #2; ships as
  0.5.2.

## [0.5.0] — 2026-05-02

### Added

- **`teamctl-ui` — terminal control room** for autonomous agent
  teams. Ships as a sibling crate (`cargo install teamctl-ui`) that
  the main `teamctl` binary can launch via the new `teamctl ui`
  subcommand wrapper. Triptych layout (Roster / Detail / Mailbox)
  with state-glyph priority indicators on every agent; live tmux
  pane streaming for the focused agent; mailbox tabs (Inbox /
  Channel / Wire) with notify-based file-watch for real-time
  updates; approvals stripe + modal that route writes through the
  existing `teamctl approve|deny` CLI to preserve `delivered_at`
  contracts; vim-keyed compose modal (`@` DM / `!` broadcast with
  per-channel picker) sending via `teamctl send|broadcast`; Wall
  and MailboxFirst alternate layouts (`Ctrl+W`/`Ctrl+M`); split-
  screen with vertical/horizontal orientation per cell (`Ctrl+|` /
  `Ctrl+-`) and `Ctrl+W q/o` chord-prefix navigation; `?` help
  overlay reading from the same keymap registry the event loop
  uses; first-launch onboarding tutorial (`t` to reopen). 110
  tests; capability-aware theming degrades cleanly to monochrome.
- `teamctl ui` subcommand in the main binary. Detects `teamctl-ui`
  on PATH and execs it with clean process handoff (Unix) or
  spawn-and-propagate-exit-code (Windows); friendly install hint
  with explicit `[y/N]` prompt when missing. `--no-prompt` flag
  for non-interactive shells / CI.
- Per-agent `effort:` field on the team-compose schema. Accepts
  `low | medium | high | xhigh | max` and flows through to
  `claude --effort` at spawn time. Strict-enum validation rejects
  typos with a clear error citing the offending agent.
- Project-as-code dogfood — teamctl ships a `.team/` directory
  inside its own repo demonstrating the `.team/` walk-up
  convention end-to-end on the project that maintains itself.

### Changed

- Approval routing invariant tightened across all decide call sites
  (CLI + Telegram callback). Status pin now precedes the
  `delivered_at` flip, with the flip gated on a successful pin —
  preserving the `undeliverable ↔ delivered_at IS NULL` invariant
  against late stale taps.
- CLI approval decisions now use a single fractional-seconds `now()`
  call threaded through both `delivered_at` and `decided_at` writes,
  matching the broker's `store::now()` precision and column affinity.
- `Supervisor::drain` extracted into `orchestrate_drain` with a
  testable trait-method poll interval (default 250ms). Drain
  contract end-to-end pinned by mock-host tests including the
  timeout=0 fast-path.
- README links retargeted at the live docs site
  (`https://teamctl.run/...`) instead of repo-relative paths that
  404 on GitHub renders.

### Fixed

- Installer prints actionable shell-tailored PATH hint when the
  install dir isn't on `$PATH` (zsh / bash / fish / fallback
  profile). Friendly without auto-mutating — copy-paste one-liner,
  never edits operator rc files.

### Notes

- TUI bug cluster from operator first-trial (T-074) — modal
  keymap discoverability, tmux color pass-through, focus-cycle
  semantics, layout-height polish — landing as 0.5.1 follow-up.
  This release ships the cascade substance; the polish iteration
  follows immediately.

## [0.4.0] — 2026-04-30

### Added

- `teamctl init` subcommand. Drops a `.team/` skeleton into the
  current directory (or any path passed as a positional). Two
  templates today — `solo` (single agent, single channel — the
  default and the right starting point for "drop teamctl into
  this project") and `blank` (empty `.team/` ready to fill in).
  Refuses to overwrite an existing non-empty `.team/` without
  `--force`. Generated files include short prose comments
  explaining what to edit next.
- Snapshot v2 + first-class `ReloadPlan`. `teamctl reload --dry-run`
  now prints the plan that *would* execute — adds, removes,
  restarts, and skips — without touching anything. Snapshot
  hashing is deterministic across runs (blake3 over normalised
  inputs), so "did this agent's config change?" stops flapping
  on Rust's per-process `DefaultHasher` salt.
- Reload drain. When an agent gets restarted by reload, its
  in-flight work is given a chance to finish first. Configurable
  via `drain_timeout_secs` in `team-compose.yaml` (default: 10
  seconds; cap 600). `0` short-circuits to instant restart for
  the cases where you really mean it.
- First-class `effort` field on the per-agent schema in
  `team-compose.yaml`. Accepts `low | medium | high | xhigh |
  max`; renders to `EFFORT=<level>` in the generated agent env
  and flows through to `claude --effort <level>`. Precedence:
  per-agent YAML > workspace `.env` > wrapper default. Strict
  enum — typos like `hgih` fail compose validation loudly with
  the offending agent named.
- Reload now persists each agent's tmux session name in the
  snapshot, so removing or restarting an agent always targets
  the right session — even if `supervisor.tmux_prefix` was
  changed between reloads.

### Changed

- `.team/` is now the canonical project root. Discovery walks up
  from cwd to the **first** `.team/` it finds and runs that team
  — npm/yarn shape, no auto-register-context magic. Operators
  `cd` into the project they're working on (or pass `-C <path>`)
  and `teamctl up` / `reload` / `ps` resolve naturally.
- Worktree-friendly runtime state. Each `.team/state/` is now
  intended to be gitignored; per-worktree runtime state lives
  inside the worktree's own `.team/`, while the `.team/` source
  layout (compose, roles, projects) is shared via git. Two
  worktrees of the same repo can run two independent agent
  teams side by side.
- `examples/*` restructured to the `.team/` convention. Every
  example now runs with `cd examples/<name> && teamctl up` —
  no `-C` flags. The `oss-maintainer` example demonstrates a
  non-default `effort:` field; new cookbook entry at
  `/cookbook/effort/` documents the field, the five accepted
  values, and the precedence rule.
- README rewritten with a project-voice "Getting started" arc
  showing the canonical flow: `cd /path/to/your/project`,
  `teamctl init`, `teamctl up`, `teamctl reload`. Frames teamctl
  as the team-of-agents that fits *into* your existing project,
  not a project scaffolder. The Mermaid diagram is gone.

### Deprecated

- `teamctl context`. The `.team/` walk-up replaces every shape
  the registered-context model used to handle. The command still
  works in 0.4.0 with a stderr deprecation note; **scheduled for
  removal in 0.5.0**. Migrate by `cd`-ing into the project root
  (or using `-C <path>`) before running teamctl commands; if you
  used `teamctl context use <path>` to pin a default, the new
  shape is to put a `.team/` in that path.

## [0.3.0] — 2026-04-30

### Added

- Per-manager bot scoping for Telegram approval routing. Approval
  cards now reach exactly one chat — the bot scoped to the manager
  that the requesting agent reports to — instead of fanning out to
  every connected bot. Routing follows the worker's direct
  `reports_to` only; deeper manager hierarchies (worker →
  team-lead → manager) are tracked as a follow-up.
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
