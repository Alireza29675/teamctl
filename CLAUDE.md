# CLAUDE.md

Repo-root governance for human contributors and any Claude Code agents working in this repo. Read this before opening a PR; the rules here apply to every change.

This file is short on purpose. The Anthropic guidance is to keep it under roughly 2K tokens and treat it like code — prune what Claude already does without being told, expand only what would otherwise produce a wrong outcome. When you add something here, run the pruning test:

> **Would removing this instruction cause Claude to make a mistake?** If no, delete it.

## What this repo is

`teamctl` is a multi-crate Rust workspace that ships docker-compose-shaped tooling for persistent agent teams. The crates cover the CLI, the schema / validate / render / supervisor core, the MCP server agents read and write the mailbox through, the Telegram bridge, and the TUI. The repo also carries an Astro Starlight docs site under `docs/`, cookbook examples under `examples/`, and a dogfood team config under `.team/` — the team that develops teamctl on teamctl, shipped in-tree because it doubles as the showcase.

User-facing intro and install path: see `@README.md`.

## Stack and entry points

Multi-crate Rust workspace, MSRV **Rust 1.78 (stable)**.

- `crates/teamctl/` — the CLI (`teamctl init | up | down | reload | status | ...`).
- `crates/team-core/` — schema, validate, render, supervisor.
- `crates/team-mcp/` — MCP server agents talk to.
- `crates/team-bot/` — Telegram bridge for user-facing managers.
- `crates/teamctl-ui/` — the TUI.

Plus `docs/` (Astro Starlight site at teamctl.run), `examples/` (cookbook recipes), and `.team/` (the dogfood team config; ships because it's the showcase).

Day-to-day commands — use `just` (defined in `justfile`), matching CONTRIBUTING.md:

```bash
just test        # cargo test --workspace
just lint        # cargo clippy -- -D warnings + cargo fmt --check
just build       # cargo build --release
```

`cargo fmt --all -- --check` is the rustfmt gate; CI runs it on every PR.

## Cross-cutting rules

1. **Every release or substantive change to teamctl must consider impact on the plugin, the TUI, the docs, and the tests. Release PRs explicitly list which of those four surfaces were touched (and why each untouched one wasn't needed).**

## Behavioral guidelines

Distilled from Karpathy's CLAUDE.md (cited below). Four habits that catch the bulk of avoidable mistakes:

- **Think before coding.** Surface tradeoffs explicitly. Name what's confusing or under-specified rather than guessing. Ask a clarifying question if the right path branches on a fact you don't have.
- **Simplicity first.** Write the minimum code that earns the test. No speculative abstractions, no future-proofing for hypothetical requirements. Before shipping a chunk, ask: *would a senior engineer call this overcomplicated?*
- **Surgical changes.** Touch only what the ticket requires. Match the surrounding style. Don't refactor adjacent code "while you're there" — file a separate ticket. Diffs that drift past their stated scope cost more in review than they save in keystrokes.
- **Goal-driven execution.** Translate the task into a verifiable success criterion before writing code. Write the test (or the manual repro) first; then make it pass. "Done" means the criterion is observably met, not "the code looks reasonable."

*Adapted from [forrestchang/andrej-karpathy-skills · CLAUDE.md](https://github.com/forrestchang/andrej-karpathy-skills/blob/main/CLAUDE.md) — read the canonical for the longer treatment.*

## Repo etiquette

Always-on rules. The longer human-facing version lives in `@CONTRIBUTING.md`.

- **Commits.** Conventional Commits, Angular form: `type(scope): subject`. Subject line only — no body, no Claude attribution, no trailers.
- **Branches.** `T-NNN/short-slug` for ticketed work; kebab-case, three or four words max, otherwise.
- **Worktrees.** Live in `.worktrees/` at the repo root. Never inside `.team/`.
- **PRs.** ≤ 400 LOC diff where possible. CI must pass. Tests included in the same PR as the code, or an explicit note in the PR body explaining why a test is infeasible.

## Pointers

Progressive disclosure — load only what the current task needs.

- `@README.md` — project overview and user-facing install path.
- `@CONTRIBUTING.md` — extended dev guide (build, lint, test, code style, PR process).
- `@decisions.md` — architectural decisions log; pointer to the formal ADR series under `docs/src/content/docs/adrs/`.
- `@patterns.md` — coding and repo patterns this project follows; accretes as the team learns.
- `@ROADMAP.md` — direction.

## Pruning

Treat this file like code: review it when something goes wrong, prune it on a regular cadence. The test is the same one from the header — *would removing this instruction cause Claude to make a mistake?* If no, delete it. Bloat causes Claude to ignore the rules that actually matter; every line that survives the pruning test earns its place. When you add something, write down the condition under which it could be removed.
