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

# Sync docs/public/install with tools/install.sh. The Astro docs site
# serves docs/public/install verbatim as https://teamctl.run/install
# (no redirect chain — see docs/public/_redirects). tools/install.sh
# is the source of truth; run this before pushing release PRs that
# touch the installer so the live endpoint actually picks up the
# change after Cloudflare Pages re-deploys.
sync-install:
    cp tools/install.sh docs/public/install
    @echo "synced docs/public/install <- tools/install.sh"
