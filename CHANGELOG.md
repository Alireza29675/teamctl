# Changelog

All notable changes to teamctl will be documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `teamctl bot setup` gains a **managed-bots** path: set up one manager bot
  (Telegram Managed Bots, Bot API 9.6) and it spawns a child bot per manager for
  you — confirm one `t.me` link each instead of a separate BotFather trip. The
  wizard mints each child's token, runs the same `/start` chat authorization, and
  writes the per-manager `interfaces.telegram` block plus the project-level
  `interfaces.telegram.manager_bot` config. The original manual-token flow is
  preserved verbatim as the other fork at the top of the wizard. (#344, #132)
- `teamctl reload --fresh` and `teamctl up --fresh` restart agents into a
  brand-new Claude conversation (re-running the bootstrap prompt) instead of
  resuming the prior session, while keeping durable on-disk files (`task.md`,
  memory, ways-of-working). The escape hatch from always-on session resume — for
  a wedged context, a bad self-compact, or token bloat. `--fresh` only affects
  agents the command actually (re)starts, and composes with the scoped
  force-restart. Claude runtime only; `codex`/`gemini` agents are skipped with a
  warning (parity gap). (#352)

## [0.8.6] — 2026-05-17

### Changed

- teamctl's Linux release binaries are now fully static (musl): they carry zero glibc dependency and install and run on any Linux — including old, minimal, or glibc-absent systems such as Debian, Alpine, Proxmox, and embedded boxes. This permanently eliminates the `GLIBC_x.xx not found` install failures that the previous `ubuntu-22.04` runner pin only deferred. (#309)

### Fixed

- New and Essentials teams now receive their Telegram messages: `bot setup` no longer corrupts the `telegram` block when writing config — a YAML-edit bug mis-nested a replaced leaf's values, breaking delivery for freshly-created teams. (#318)
- `teamctl bot setup` no longer echoes the Telegram bot token as it is typed or pasted. The prompt now reads it with terminal echo disabled, keeping the credential out of the screen, scrollback, screen-shares, and recordings. (#315)
- Agent env-file loading is hardened: env values are no longer passed through the shell, so a value containing spaces or glob characters (`*`, `?`) can no longer mangle the agent's environment or pull unintended files into it. (#307)
- The `teamctl ui` Detail pane now fits the agent's terminal output correctly — the inner tmux session is sized with `resize-window`, so captured content no longer overflows or clips inside a smaller pane. (#317)
- Submitting in the `teamctl ui` compose editor now works on all terminals: plain Enter in Normal mode submits, fixing send on default-mode terminals (xterm, Terminal.app, GNOME Terminal) where the previous Alt/Ctrl+Enter chord never fired. (#316)

## [0.8.5] — 2026-05-16

### Added

- `request_approval` gains multi-option interactive decisions: a new `options` parameter and a new `decided` status. A manager can offer the user a choice list (not just approve / deny) and read back which option was picked. Touches the `team-mcp` / `team-bot` / `team-core` approval schema. (#301)
- `teamctl adjust` — a CLI shim that execs `claude /teamctl:adjust`, mirroring how `teamctl init` enters its interactive skill. (#248)
- Interactive substrate shared by the `/teamctl:init` and `/teamctl:adjust` skills — a conversational layer the skills drive. (#247)
- `teamctl init` pre-flight: a `.team/` guard so an existing team isn't clobbered, a dependency pre-flight check, and a Guided / Essentials / Blank mode picker. (#297)
- Headless `claude` agents block interactive tools (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`) by default, so a headless agent can't stall on a prompt it can't receive. Thanks to outside contributor Hamed Fathi. (#246)

### Changed

- `teamctl ui` mailbox channel tab now prefixes each row with the channel name and sender, so cross-channel traffic is legible at a glance. (#251)
- README "Start a team" now leads with `teamctl init` instead of the `claude` slash-command, and lists the Guided / Essentials / Blank picker options. (#239, #250)
- `/teamctl:release` skill tightened to a shorter-is-better shape with major / minor / patch templates and a verify-handle thanks rule, per owner directive. (#234)

### Fixed

- cargo-dist linux release binaries no longer require `GLIBC_2.39`. The linux build runners are pinned to `ubuntu-22.04` (glibc 2.35), restoring the binary install path on Debian 12, Ubuntu 22.04, Proxmox, and most stable LTS Linux. (#296)
- The installer and docs now reference the correct Claude Code plugin id `teamctl@teamctl` for `claude plugin update`, so the plugin auto-update path works instead of failing on an unrecognized plugin name. (#298)
- `reply_to_user` rejects oversized text / caption at the MCP boundary instead of failing deeper in the bridge. (#293)
- `team-bot` setup no longer fails on a just-edited compose file when run a second time — it reuses the parsed compose instead of re-reading from disk. (#245)
- `team-bot` replies with a configuration hint when a voice note arrives but speech-to-text is unconfigured, instead of silently dropping it. (#237)
- macOS CI purges pre-seeded rust stub shims before the toolchain install and pins the runner to `macos-14`, fixing a red `main` pipeline. (#285)

## [0.8.4] — 2026-05-12

### Removed

- `/teamctl:release` skill is no longer shipped via the Claude Code plugin. It was an internal release-authoring tool, not a user-facing feature; previously bundled in `plugins/claude-code/skills/release/` by mistake. Moved to `.claude/skills/release/SKILL.md` (project-local, not part of the plugin install). Users of the `teamctl` plugin won't see it after their next `teamctl update`. (#228 follow-up)

## [0.8.3] — 2026-05-12

### Added

- `examples/gastown-in-teamctl/` — a positioning artifact expressing Gas Town's seven-role formation (mayor / crew / refinery / witness / polecats / deacon / dog) as a teamctl team. README walks the mapping from Gas Town's opinionated frame to teamctl's unopinionated declarative layer, including the reinterpretations (polecats as a fixed pool, beads-vs-mailbox as a deliberate architectural trade-off, formulas as a vision-track gap). Credits Steve Yegge's original Medium piece. (#229, #230)
- `teamctl ui` Agents pane now renders `reports_to` relationships as a nested tree with Unicode glyphs (and ASCII fallback under `NO_COLOR` / monochrome terminals). Selection stays sticky-on-id; flat teams render byte-identically. (#211, #225)

### Changed

- `teamctl ui` mailbox Sent tab now shows `[→recipient]` instead of the redundant `[sender]` (always self for the focused agent). Recipient resolves to display_name for agents, `#name` for channels, and the verbatim id for user surfaces like `user:telegram`. Other tabs unchanged. (#231, #232)
- `/teamctl:release` plugin skill grew durable shape variants (major / minor / patch templates) and a verify-handle thanks rule, so future releases pull a tighter shape and never ship a guessed contributor handle. (#228)

## [0.8.2] — 2026-05-12

### Added

- Bottom status bar in `teamctl ui` — left side shows the team-root path, right side shows live CPU% + RAM%, with a center slot reserved for per-agent surfaces. Uses the `sysinfo` crate trimmed to a system-only feature set. (#209, #217)
- Per-agent claude rate-limit indicator in the bottom-bar center slot — shows `limit Xh Ym` / `limit Xm Ys` for the focused agent when claude has signalled a rate-limit window. Preview-gated behind `TEAMCTL_UI_RATE_LIMIT_INDICATOR=1` (any non-empty value opts in; default OFF; read once at TUI start). (#212, #218)
- CI now runs a dedicated `macos-latest` cell so darwin-flavor regressions get caught at PR time, plus a `bash --posix -O compat32 -n` parse-check on the existing Linux runner that surfaces the bash-3.2 class of wrapper-script bugs without waiting on a macOS minute. Both motivated by the T-190 regression that bit 0.8.0 on macOS. (#192, #193, #205)

### Changed

- `teamctl init` retires the `solo` template. New three-option picker: **Guided** (interactive conversation, default for interactive runs), **Essentials** (two-project layout — blank `main.yaml` plus an `ops` project with a `builder` agent that has scoped authority over `main`; default for `--yes`), and **Blank** (kept as the minimal scaffold). `--template guided --yes` rejects cleanly. ADR-0004 carries a supersede note. New cookbook walkthrough on `teamctl.run`. (#206, #216)
- `teamctl update` now reinstalls `teamctl-ui` alongside `teamctl`, `team-mcp`, and `team-bot` on the cargo-install path. The four crate names are centralised as a single constant so the bug class can't recur silently. Stale 3-crate command references in docs and the Claude Code init template were swept in the same wave. (#188, #204, #207)
- `teamctl ui` splash screen now has a single blank line between the ASCII logo and the version/team-status line so the layout is less cramped. (#208, #214)
- `teamctl ui` reflows the focused agent's tmux pane size to match the `Detail` rect on every frame (cache-gated so steady-state frames don't fork tmux). The claude TUI now visibly tracks teamctl-ui resizes. Scoped to the Triptych layout; Wall + MailboxFirst sweeps will land separately. (#199, #210)

### Fixed

- `teamctl whatsnew` no longer dumps the cargo-dist per-crate install tables verbatim after the curated release prose; the renderer now detects the `# <crate> <semver>` heading injected by cargo-dist and truncates there. (#197, #200)
- `teamctl whatsnew --since <ver>` no longer emits a redundant `v<from> → v<to>` range frame plus per-version subheader when the resolved range contains a single version. (#198, #200)
- `teamctl whatsnew` no longer renders a double blank line under the frame when the body is empty or got truncated to empty by the cargo-dist heading detection. (#201, #203)
- `team-mcp` channel-notify watcher no longer hits a lost-wake race against macOS scheduling. `tokio::sync::Notify::notify_waiters` was swapped for `notify_one` so the `notifications/initialized` signal buffers a permit if the watcher hasn't parked yet. Added a Linux-deterministic regression pin via a test-only env var. (#215)

## [0.8.1] — 2026-05-11

### Fixed

- `agent-wrapper.sh` now starts cleanly on macOS (bash 3.2). The previous `${BOOTSTRAP_PROMPT:=...}` parameter-expansion form tripped a parser bug in bash 3.2 (macOS `/bin/sh`) when the default contained escaped backticks, causing agents to abort immediately after `teamctl up`. Rewritten as a plain `if [ -z ... ]` conditional that parses identically on bash 3.2, bash 4+, and dash. Latent since T-104. (#190, #191)
- `agent-wrapper.sh` now guards `$CLAUDE_SESSION_ID` and `$CLAUDE_SESSION_NAME` under `set -u`. Previously, if either env var failed to render into the agent env file, the wrapper would abort silently. (#190, #191)

## [0.8.0] — 2026-05-11

### Added

- Voice notes on Telegram are transcribed to text via Groq STT and forwarded to the manager (#105).
- File attachments in compose: path-input overlay in the TUI plus agent reads via the new `read_attachment` MCP tool (#139, #147).
- Stream-keys (`Ctrl+E`) in the mailbox pane forwards subsequent keystrokes straight to the focused agent's tmux pane (#114).
- `Sent` tab in the mailbox pane shows the focused agent's outbox alongside `Inbox`, `Channel`, and `Wire` (#127).
- Mouse-wheel scrolling in `teamctl ui` routes through the focused pane — copy-mode history on `Detail`, agent step on `Roster` (#163).
- `teamctl sessions` lists every tmux session across projects in one view (#112).
- `teamctl update` refreshes the Claude Code plugin alongside the binaries (#148).
- `role_prompt` accepts a list of markdown paths, concatenated in declared order at agent boot (#142).
- Per-role `ways-of-working.md` convention — gitignored, lazy-created, read at the start of every tick (#136, #143).
- `compact_self` MCP tool — agents tidy their own context without an external nudge (#128).
- `show_typing` MCP tool — managers send a "typing…" indicator to Telegram (#123).
- Lazy inbox delivery: channel notifications arrive as short stubs; `/readnow` prefix bypasses for per-message full-body delivery (#113).
- Always-on session resume via deterministic per-agent UUID (#151).
- Daily update-availability nudge on `teamctl status` and `teamctl up` when a newer release is on GitHub (#150).
- Mailbox tab navigation via arrow keys (#125).
- Per-project scope for `teamctl up` / `down` / `reload` (#135).
- `teamctl init` replaces the template picker with a domain-discovery conversation (#157).
- Telegram bot renders a markdown subset to Telegram HTML parse mode (#137).
- HITL approver name is derived from the Telegram callback sender (#120).

### Fixed

- HITL approval outcome no longer hardcodes the approver name (#120).
- Telegram bot html-escapes agent identifiers in HITL cards and attribution lines (#145).
- `agent-wrapper.sh` auto-confirms the bypass-permissions and usage-limit dialogs so first launches and rate-limit windows don't strand agents (#121). Thanks to Hamed Fathi for surfacing the bypass-permissions deadlock.
- `claude` TUI renders at the full pane size instead of `80×24` (#99).
- `teamctl ui` Roster column renamed to Agents; Triptych Detail/Mailbox split tuned to 60/40 (#95, #96).
- Telegram bot restricts `fence_marker` language tags to `[A-Za-z0-9_-]` (attribute-injection hardening) (#154).
- Deflake `channels_notify` under loaded CI runners (#119, #144).
- Deflake `real_scanner_for_spec_timeout_returns_rejected` (#152).

### Changed

- `tools/install.sh` is now served as a static asset on `teamctl.run/install` and regenerated on every docs build (#92, #93).
- `tools/install.sh` bundles `teamctl-ui` and offers the Claude Code plugin install/update on interactive runs (#97).
- README rewritten with an examples-first flow (#161).
- Docs site polish sweep: legacy landing retired, modernized guides, ways-of-working page (#162).
- Six relatable example teams replace the SaaS-only example (#165).
- New "How to think about agent teams" concept page (#153).
- Channels-with-ACLs explainer added to the concepts section (#166).

## [0.7.3] — 2026-05-08

### Added

- **`react_to_user` MCP tool** (T-086-E). Manager agents can now react
  to operator Telegram messages with an emoji from a curated allowlist
  (`👍 👎 ❤️ 🎉 👀 🤝 👨‍💻`). Reactions ride the `kind`+`structured_payload`
  discriminator added in 0.7.2 — the bot dispatcher reads `kind =
  "reaction"` and routes to Telegram's `setMessageReaction` API
  instead of `sendMessage`. Off-allowlist emoji surface a clean MCP
  error rather than reaching Telegram and getting silently rejected.
  Manager-gating preserved — worker agents cannot react.
- **`reply_to_user` reply-threading via `reply_to_message_id`**
  (T-086-B). Agents can now thread replies under the operator's
  prior message in Telegram by passing the inbound message's
  `telegram_msg_id` as `reply_to_message_id`. Outbound rows carry
  the field forward; `team-bot` attaches `reply_parameters` on
  `sendMessage` / `sendPhoto` / `sendDocument` so the reply visually
  nests under the parent in the chat client. Multi-content calls
  (text + image in one tool invocation) share one threading target
  so the operator sees both attachments under the same parent, not
  split across threads. Back-compat preserved: callers omitting
  the field land messages as fresh posts as before.
- **Inbound media handling** (T-086-C). `team-bot` now detects
  inbound photos and documents from the operator, downloads them to
  a per-project disk cache (`<media_root>/<project>/<row_id>.<ext>`),
  and writes a structured mailbox row (`kind = "image"` / `"file"`,
  `structured_payload = {path, mime, size_bytes, caption?}`). Agents
  read file bytes from the disk path via their runtime's vision
  plumbing. Two-phase SQL pattern: insert `media_pending` placeholder
  → download → UPDATE to final kind. Network/disk-full failures
  surface as `media_error` rows with the verbatim cause (R12) so the
  operator's reply prompt has a real diagnostic instead of silent
  drop. Documents whose mime starts with `image/` are classified as
  `kind = "image"` so vision plumbing picks them up — operators
  often upload PNG/GIF as document to avoid Telegram's JPEG
  recompression on `photo`.
- **Version line in `teamctl --help`** (T-091). The help page's
  first line now shows `teamctl <version>` so operators can
  disambiguate "which version am I running" without a separate
  `--version` round-trip. Pulls from `CARGO_PKG_VERSION` so it
  auto-tracks the workspace version at release time.

### Fixed

- **`teamctl-ui` was never published to crates.io** (T-095). The
  `teamctl ui` command resolves to `cargo install teamctl-ui` to
  install the TUI on demand (intentionally opt-in to avoid pulling
  the ratatui+crossterm dep tree into every install), but the
  publish-crates workflow shipped only the four core crates and
  skipped `teamctl-ui` — so `teamctl ui` failed with `could not find
  teamctl-ui in registry crates-io`. The workflow now publishes
  `teamctl-ui` alongside the others; v0.7.2 was manually backfilled
  to crates.io to unblock the operator surface, and v0.7.3 onward
  ships through the workflow.
- **`tools/install.sh` tag-name parser broken on single-line GitHub
  API responses** (T-085). The previous parser used a greedy
  `sed -E 's/.*"([^"]+)".*/\1/'` after `grep '"tag_name":'`, which
  works on pretty-printed JSON but captures the *last* quoted string
  on a line — buried deep in the release `body` field's CHANGELOG
  markdown when the API returns compact (single-line) JSON. The
  installer then resolved `$VERSION` to garbage and failed at the
  tarball fetch. Parser now anchors on the `tag_name:` field name
  with `jq` when available and a tighter `sed -nE 's/.*"tag_name":
  *"([^"]+)".*/\1/p'` fallback otherwise. Same behaviour on
  pretty-printed input; correctly handles compact JSON.
- **macOS-14 installer-smoke check rate-limited on shared runner
  IPs** (T-088, folded into T-085). The smoke workflow ran
  `install.sh` whose `curl https://api.github.com/repos/.../releases/latest`
  hit the 60-req/hour unauthenticated-IP rate limit on shared CI
  runner pools. Workflow now pre-resolves `TEAMCTL_VERSION` via
  authenticated `gh release view` (5,000 req/hour for authenticated
  requests) and exports it as an env var so install.sh skips its
  own latest-resolver. Linux runners weren't affected in practice,
  but the auth fix covers them too.

### Changed

- **`install.sh` served as a static asset on teamctl.run/install**
  (PR #86, post-0.7.2 hot-fix). The previous `/install` endpoint
  did a redirect to the raw GitHub URL, which Cloudflare Workers
  occasionally responded to with cached redirect chains that
  confused `curl -fsSL`. The Astro docs site now serves
  `tools/install.sh` directly as `/install` — no redirect, no
  caching surprises. `tools/install.sh` is now the source of truth
  for the install path.
- **`publish-crates.yml` workflow gains `workflow_dispatch`
  trigger.** The release-tag-push trigger remains; the manual
  trigger is for ad-hoc backfill scenarios (e.g. publishing a
  newly-added crate against the current main branch without
  cutting a fresh release). Version-match guard now scoped to
  `push` events only.

## [0.7.2] — 2026-05-04

### Added

- **Outbound media via `reply_to_user`** (T-086-A). Manager agents can
  now send images and files to operators through the Telegram bot, with
  optional captions. The `reply_to_user` MCP tool gains optional `image:
  {source: "path"|"url", value, caption?}` and `file: {...}` fields.
  Text-only callers continue working unchanged. Multi-content per call
  yields separate Telegram messages in order; the response carries both
  legacy `id` and a new `ids` array. Path-source files validated for
  existence + ≤50MB Telegram bot limit + image extension allowlist
  (jpg/jpeg/png/webp/gif). Manager-gating preserved — worker agents
  cannot send media.
- **Slash-passthrough to tmux session** (T-086-G), Claude-Code-only.
  Telegram messages starting with `/` (e.g., `/clear`, `/compact`,
  `/cost`) bypass mailbox routing and get typed directly into the
  manager's tmux session via `tmux send-keys`. Feature-gated on
  `runtime: claude-code`; non-CC managers respond with a clear reject
  message naming the actual runtime. Trust posture: operator owns the
  bot (single-operator deployment shape); arbitrary text typed via
  slash-passthrough runs at agent privilege — same trust boundary the
  operator already extends to the agent's tmux via direct ssh / tmux
  attach.
- **Telegram bot autocomplete via `setMyCommands`** (T-086-H). Each
  manager-scoped CC bot registers a curated set of 12 Claude Code slash
  commands (`/clear`, `/compact`, `/cost`, `/help`, `/init`, `/mcp`,
  `/model`, `/permissions`, `/resume`, `/review`, `/status`, `/vim`)
  on startup so the operator gets an autocomplete menu when typing
  `/`. Pairs with the slash-passthrough above for clean operator UX.
  Hyphenated CC commands and login flows excluded (Telegram's bot-API
  restricts command names to `[a-z0-9_]`; login flows are awkward over
  chat). Best-effort registration: a Telegram API failure logs a
  warning and bot startup continues; slash-passthrough still works
  manually. Non-CC managers register no commands (clean degrade).

### Fixed

- **TUI chord-arm casing-fold across remaining Ctrl+letter chords**
  (T-082). Following 0.7.1's Ctrl+W/M case-fold fix, sweeps the same
  bug class for `Ctrl+H` / `Ctrl+J` / `Ctrl+K` / `Ctrl+L` (split
  cycling) and `Ctrl+Q` (close-focused-split). All five chord arms
  now accept both lowercase and uppercase Char so they survive
  CapsLock + Shift+Ctrl variants. Plain-q quit-confirm guarded with
  `is_empty()` modifier check so plain-q doesn't shadow `Ctrl+q` close-
  focused-split.

## [0.7.1] — 2026-05-04

### Changed

- Plugin slash commands renamed: `/teamctl:teamctl-init` → `/teamctl:init`,
  `/teamctl:teamctl` → `/teamctl:adjust`. Operators upgrading must use the
  new invocation forms.
- `/teamctl:init` Stage 6 now defers to `teamctl bot setup` rather than
  wrapping the BotFather/token/chat-id wizard inline. Onboarding now points
  the user at the CLI wizard for Telegram setup instead of running it
  through the model.
- README rewritten to a tighter two-section onboarding shape (Interactive
  Setup with the Claude Code plugin + Manual setup), -53 lines net.
  Comparison content moved to <https://teamctl.run/compare/>. Hero motto
  updated to *"Run real AI agent teams from one YAML. Each agent is a
  long-lived CLI process."* with a body soft-analogy for readers who know
  docker-compose.

### Fixed

- TUI layout-switch chord `Ctrl+W` / `Ctrl+M` now triggers correctly when
  CapsLock is engaged or Shift is held alongside Ctrl. Prior arms only
  matched lowercase `Char('w')` / `Char('m')`, so the chord died silently on
  uppercase variants.
- TUI DM compose modal accepts `Alt+Enter` as a universal send chord on
  standard terminals (xterm, Terminal.app, tmux). Prior `Ctrl+Enter` was the
  only wired chord, but standard terminals strip the Control modifier from
  Enter so the chord was unreachable except on kitty-keyboard-protocol
  terminals. `Ctrl+Enter` still works on those terminals.

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
