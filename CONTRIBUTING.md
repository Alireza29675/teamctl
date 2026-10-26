# Contributing

Thanks for looking. teamctl is early — the fastest path to a useful contribution is a bug report or a small, focused PR.

## Development

```bash
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
just test        # all workspace crates and integration tests
just lint        # workspace clippy with warnings denied, plus formatting
just build       # debug binaries and test targets
just release     # release binaries
```

Minimum supported Rust version: **1.86** (stable). Daily development runs on `stable` (per `rust-toolchain.toml`); to check the floor explicitly:

```bash
just msrv-check  # builds the shipped CLI crates on the MSRV toolchain
```

It reads the floor from `Cargo.toml`'s `rust-version`, installs that toolchain if needed, and fails if the code or a dependency requires a newer compiler. CI enforces the same floor (a build matrix leg and the publish workflow). `teamctl-ui` carries a higher floor and is excluded, matching the install path.

## Verification runbook

Run commands from the repository root. Last verified: 2026-10-10.

- Install `tmux` to exercise the real lifecycle test. It uses a private socket, temporary HOME/root, and fake runtimes. It never contacts a model provider. Without tmux it reports that the test was skipped; CI installs tmux.
- `just lint`, `just test`, and `just msrv-check` are the local gates. The optional UI requires Rust 1.88; shipped CLI crates retain 1.86.
- Check managed shell assets with `sh -n crates/teamctl/assets/agent-wrapper.sh` and `sh -n crates/teamctl/assets/boot.sh`. On macOS also use `/bin/bash --posix -n` for the actual 3.2 parser.
- To exercise the ignored real UI resize contract explicitly, run `cargo test -p teamctl-ui pane_resize -- --ignored --nocapture`. It creates an isolated tmux socket.
- Documentation requires Node **22.12 or newer**. Run `npm ci`, `npm audit`, and `npm run build` from `docs/`. `just docs` builds an already installed dependency tree. Preview locally with `npm run preview`; deployment stays in the docs workflow.
- Optional messaging adapters: from `adapters/`, use Node22.12+ and run `PUPPETEER_SKIP_DOWNLOAD=true npm ci --ignore-scripts`, `npm test`, `npm run check` and `npm audit`. Normal tests use disposable fake processes and mock providers; no accounts, browser or model quota. CI is configured to run these checks on Linux and macOS and provides Node22 for Rust↔SDK fixtures. See [protocol and SDK](adapters/README.md). Explicit browser smoke commands are documented separately in the WhatsApp README.
- Dependency refreshes must retain the MSRV gates. Cargo's incompatible-Rust-version fallback is enabled; metadata alone cannot establish compatibility. `psm` is locked to 0.1.24 because newer releases pull `ar_archive_writer` syntax that fails Rust 1.86.

For a live runtime smoke, use a disposable compose root and private tmux socket, a single agent, and a bounded prompt. Verify the intended root before starting. Stop that fixture with `teamctl --root <fixture> down`, then kill only its private server. Keep provider smoke tests outside the normal suite so tests do not spend operator quota. See [architecture and extension seams](ARCHITECTURE.md).

## Code style

- `rustfmt` defaults, enforced in CI.
- `clippy` warnings are errors.
- `tracing` for logs — never `println!` outside of CLI user-facing output.
- Errors: `anyhow::Result` at binary edges, `thiserror` at library boundaries.
- No panics outside `main.rs` / `#[tokio::main]`.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/):

```
feat(teamctl): add `teamctl bridge open` command
fix(team-mcp): honor message TTL on inbox_peek
docs(concepts): explain bridge expiry semantics
```

Subject line only — no body, no trailers.

## PR process

- One phase at a time is too big; break into sub-PRs of ≤ 400 LOC diff where possible.
- Every PR must pass CI and include either a test or an explicit note why a test is infeasible.
- Every PR is reviewed by the maintainer before merge.

## Proposing design changes

Open an issue first. If the change is load-bearing (new runtime adapter, a break in the compose schema, a change to HITL semantics), submit an ADR under `docs/adrs/`.
