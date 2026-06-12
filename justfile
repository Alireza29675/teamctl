# teamctl dev tasks
#   just <task>

default:
    @just --list

# Build all crates in debug mode.
build:
    cargo build --all-targets

# Build release artifacts.
release:
    cargo build --release

# Run the full test suite.
test:
    cargo test --all

# Lint (clippy + fmt check). CI mirrors this.
lint:
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings

# Verify the shipped CLI crates build on the declared MSRV floor.
msrv-check:
    #!/usr/bin/env bash
    set -euo pipefail
    # Read the floor from Cargo.toml so the number is never duplicated, install
    # that toolchain if missing, and build the default-members only: `teamctl-ui`
    # has a higher floor and is excluded, matching the CI MSRV leg + the publish
    # path (.github/workflows/publish-crates.yml runs the same check).
    msrv="$(grep -E '^rust-version = ' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
    echo "Verifying MSRV floor: $msrv"
    rustup toolchain install "$msrv" --profile minimal --no-self-update || true
    cargo "+$msrv" build --locked

# Auto-format.
fmt:
    cargo fmt --all

# Quick dev loop: watch and re-run tests.
dev:
    cargo watch -x 'test --all'

# Build docs locally (Phase 8 will fill this in).
docs:
    @echo "Docs site is introduced in Phase 8."
