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

# Auto-format.
fmt:
    cargo fmt --all

# Quick dev loop: watch and re-run tests.
dev:
    cargo watch -x 'test --all'

# Build docs locally (Phase 8 will fill this in).
docs:
    @echo "Docs site is introduced in Phase 8."
