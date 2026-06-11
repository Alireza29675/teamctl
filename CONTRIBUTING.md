# Contributing

Thanks for looking. teamctl is early — the fastest path to a useful contribution is a bug report or a small, focused PR.

## Development

```bash
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
just test        # cargo test
just lint        # cargo clippy -- -D warnings + cargo fmt --check
just build       # cargo build --release
```

Minimum supported Rust version: **1.86** (stable). Daily development runs on `stable` (per `rust-toolchain.toml`); to check the floor explicitly:

```bash
just msrv-check  # builds the shipped CLI crates on the MSRV toolchain
```

It reads the floor from `Cargo.toml`'s `rust-version`, installs that toolchain if needed, and fails if the code or a dependency requires a newer compiler. CI enforces the same floor (a build matrix leg and the publish workflow). `teamctl-ui` carries a higher floor and is excluded, matching the install path.

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
