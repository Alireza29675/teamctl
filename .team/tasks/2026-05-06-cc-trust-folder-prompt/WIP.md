# T-090 — WIP write-down on park

> Filed: 2026-05-09 by dev1
> Trigger: project-owner halt (pm msg 2859 / project-owner msg 2856) → eng_lead park dispatch (msg 2861) mid-implementation. Capacity-event context: dev2/dev3/qa dead since msg 2831; dev1 was the single dev for the T-099→T-090 queue.
> Resume entry point: read `PHASE-1.md` + this file + the branch diff.

## Branch state

- Branch: `T-090/cc-trust-folder-prompt` (local only as of write-down)
- Worktree: `.worktrees/T-090/`
- Base: origin/main `37cd8b1` (Merge PR #97 / T-099)
- Head: pinned by the `wip(T-090):` commit referenced in `.team/state/dev1/log.md`'s T-090 entry. Pre-park gates were all green locally — workspace tests 265 (44 + 7-new-claude_trust + 17 cli + 98 + 54 + 21 + various sub-bins), fmt clean, clippy `-D warnings` clean.

## Edits already on the branch

- **NEW** `crates/teamctl/src/claude_trust.rs` — shared helper module.
  - `pub fn pre_trust_cwd(cwd: &Path) -> Result<()>` — single-cwd convenience for `cmd/init.rs`. Canonicalizes via `cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf())` (the `unwrap_or_else` keeps the raw path if canonicalize fails, matching the old `up.rs` shape's `.ok().or(Some(cwd))`).
  - `pub fn pre_trust_cwds(cwds: &BTreeSet<PathBuf>) -> Result<()>` — multi-cwd entry used by `cmd/up.rs`. Reads `$HOME` once, delegates to private `write_trust_state`. Caller is responsible for canonicalization (each call site has different rules about how relative paths resolve — `up` resolves agent cwds against `compose.root`, `init` only ever has a single just-scaffolded folder).
  - Private `fn write_trust_state(cwds, home: &Path)` — splits the $HOME read off so unit tests target a hermetic temp `$HOME` without racing the process env.
  - 7 unit tests under `#[cfg(test)] mod tests`: empty-input no-op, fresh-home create, existing-projects-preserved, idempotent-on-already-trusted, flips-untrusted-to-trusted, malformed-config-recovery, canonicalize-via-real-path. Use `tempfile = "3"` (already in teamctl dev-deps).
- **`crates/teamctl/src/main.rs`** — `mod claude_trust;` added above `mod cmd;` so both the binary and `cmd/up.rs` + `cmd/init.rs` can reach it via `crate::claude_trust::*`.
- **`crates/teamctl/src/cmd/up.rs`** — `ensure_claude_trust(compose: &Compose)` shrunk to ~20 lines: keeps the compose-iterates-claude-code-agents filter + cwd-resolve logic (up-specific), then delegates the actual write to `claude_trust::pre_trust_cwds(&cwds)`. Behavioral invariant preserved — same ordering, same cwd-canonicalization fallback, same "no banner if no fresh writes" output.
- **`crates/teamctl/src/cmd/init.rs`** — added a single `crate::claude_trust::pre_trust_cwd(&parent)?;` line in `pub fn run` between the scaffold-files loop and the `✓ <target> scaffolded.` print. `parent` is the right path because:
  - `teamctl init my-team` → `parent = cwd.join("my-team")` (operator will `cd my-team` then `claude` per the existing `Next:` hint).
  - `teamctl init` → `parent = cwd` (in-place scaffold).
  Both match the spec's "the cwd that will host a Claude Code session" framing.

## What was about to land next (NOT yet on the branch)

In rough sequence:

1. **CHANGELOG `[Unreleased]` Fixed entry** describing the new `teamctl init` pre-trust behavior. Format follows the project's other `Fixed` entries (anchor on the operator-visible symptom + reference T-090). Surface to flag: it's a UX-improvement, no behavior-change for non-CC users.
2. **Cookbook / docs entry update** — search `docs/src/content/docs/` for first-run guidance and append a one-paragraph mention that `teamctl init` pre-accepts CC's trust prompt for the folder it scaffolds. PHASE-1 acceptance #7 explicitly asks for this; saw the relevant first-run file paths flagged during spec read but didn't commit which doc gets the addition (likely `docs/src/content/docs/getting-started/...` or the `concepts/projects.md` page). Resume action: `grep -rln "trust\|claude\|prompt" docs/src/content/docs/` first.
3. **Manual smoke trace** to log into the PR description: `mkdir /tmp/t-090-smoke && cd /tmp/t-090-smoke && teamctl init --yes && cat ~/.claude.json | jq '.projects["/tmp/t-090-smoke"]'` and confirm `hasTrustDialogAccepted: true` lands. (Acceptance #6 — manual smoke, not automated.) The `~/.claude.json` change can be reverted afterwards with a small jq one-liner; documented in resume notes.
4. (Belt-and-braces) Confirm `cmd/up.rs::ensure_claude_trust` still emits the `trust · ...` banner identically post-refactor — the helper at `claude_trust::write_trust_state` prints the same `eprintln!("trust · auto-accepted Claude Code workspace trust for {path}");` line, so the banner stays. No regression-pin test for the banner because the existing teamctl tests didn't pin it either; surgical scope.

## Reasoning that isn't obvious from the diff

- **Why `claude_trust.rs` lives at `crates/teamctl/src/` and not `crates/team-core/`** — PHASE-1 explicitly leans toward "`crates/teamctl/src/claude_trust.rs` (no team-core consumers today; YAGNI)." The trust-write reads `$HOME`, mutates a user-config file, and prints a banner — all bin-side concerns. team-core is the schema/validate/render/supervisor layer; pulling host-side filesystem mutation in there violates its abstraction. Lift-to-team-core only if a second non-teamctl consumer surfaces.
- **Why the multi-cwd entry takes `&BTreeSet<PathBuf>` rather than `IntoIterator<Item = PathBuf>`** — keeps the call shape from `cmd/up.rs` byte-identical (it already collects to `BTreeSet<PathBuf>`). Generic iterators were briefly considered but abandoned: extra type-parameter noise for zero call-site benefit, and the BTreeSet shape carries de-dup + deterministic-ordering for the banner output.
- **Why `pre_trust_cwd` canonicalizes inside the function rather than at the call site** — `cmd/init.rs::run` doesn't have a natural canonicalize step today (it just builds `parent = cwd.join(name)` and writes scaffold files into `target = parent.join(".team")`). Pushing canonicalize into the helper means the init call site stays a one-liner; up.rs already canonicalizes per-cwd in its filter chain so it skips the convenience wrapper.
- **Banner UX in init.rs** — placed the `pre_trust_cwd` call BETWEEN the scaffold-files loop and the `✓ <target> scaffolded.` print, so banner-output order on a fresh run reads:
  ```
  trust · auto-accepted Claude Code workspace trust for /home/.../my-team

  ✓ /home/.../my-team/.team scaffolded.

  Next:
    cd my-team
    cp .team/.env.example .team/.env   # edit secrets
    ...
  ```
  The trust banner precedes the scaffold success because it's a distinct (per-machine, one-time) write; placing it after `✓` would imply "the scaffold also did this" which is technically true but the banner reads more naturally as preamble-not-detail. Order is debatable; if a reviewer prefers the banner inside or after the success block, it's a one-line move.

## Open questions / blockers surfaced during impl

- **Resume question for PR body**: should the cookbook entry land in this PR or be filed as a follow-up if reviewer prefers narrower diff? PHASE-1 acceptance #7 lists it as part of T-090; default = same-PR per CLAUDE.md "tests in the same PR as the code" spirit (docs in the same PR as the behavior change).
- **Manual-smoke evidence shape** — the PR body wants a trace; my plan was to inline a 3-line `jq` output snippet showing the trust entry landing for a fresh `/tmp/...` folder. Reviewer preferences may differ; safe default.
- **No automated integration test for `cmd/init.rs` writing trust state** — would require either (a) end-to-end spawn of `teamctl init --yes` against a temp $HOME, OR (b) extracting init's scaffold-then-trust loop into a testable function. Option (a) is doable via `Command::new(env!("CARGO_BIN_EXE_teamctl"))` + `.env("HOME", tempdir)`; option (b) is a refactor beyond the lift. Phase-1 didn't ask for it; the existing 7 unit tests in `claude_trust.rs` cover the trust-write contract exhaustively, and `cmd/init.rs` is a one-line call site. Defer unless reviewer pushes back.

## Resume path

1. Read `PHASE-1.md` + this `WIP.md` + `git diff origin/main...T-090/cc-trust-folder-prompt` (cumulative diff with the wip commit).
2. Run `cargo test --workspace` — should still be 265 green.
3. Land the four "about-to-land-next" items above (CHANGELOG → cookbook → manual-smoke trace → optional banner-position tweak).
4. Squash or amend the `wip(T-090):` commit into a final Angular `feat(teamctl): ...` shape before DM'ing branch-ready (or land follow-up commits and squash before merge). Convention from prior PRs is to squash to a single commit when the working stack is one logical unit.
