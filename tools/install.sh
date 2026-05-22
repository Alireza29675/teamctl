#!/bin/sh
# teamctl installer — downloads the latest release tarballs for the current
# platform (one per binary: teamctl, team-mcp, team-bot, teamctl-ui), verifies
# sha256s, and drops the binaries in ~/.local/bin. When Claude Code is on
# PATH and the script runs under a real TTY, it also offers to install the
# teamctl Claude Code plugin (or silently updates it if already installed).
#
# Usage:
#     curl -fsSL https://teamctl.run/install | sh
#     bash -c "$(curl -fsSL https://teamctl.run/install)"   # interactive plugin prompt
#     curl -fsSL https://teamctl.run/install | sh -s -- --version v0.2.9
#
# Environment overrides:
#   TEAMCTL_INSTALL_DIR   target directory (default: ~/.local/bin)
#   TEAMCTL_VERSION       version tag to install (default: latest)
#   TEAMCTL_VERIFY        set to 0 to skip sha256 verification (offline/dev only)
#
# https://teamctl.run/install serves this exact script: the docs.yml CI
# workflow regenerates docs/public/install from tools/install.sh on every
# docs build and Cloudflare Pages publishes it (no redirect, no cargo-dist
# involvement). tools/install.sh is the single source of truth — for the
# live endpoint, for `teamctl update`'s shell path, and for local installs.

set -eu

REPO="Alireza29675/teamctl"
INSTALL_DIR="${TEAMCTL_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${TEAMCTL_VERSION:-latest}"
VERIFY="${TEAMCTL_VERIFY:-1}"

# Minimum Claude Code version for the teamctl plugin install path.
# T-118 / T-174 lineage: `claude --session-id` had substrate bugs
# pre-2.1.141 that confused supervised `teamctl up` resume. Installing
# the plugin against an older runtime sets the operator up for a
# confusing first-run failure; the binary install (above) is NOT gated
# on this — only the Claude Code plugin step is. Bump requires a
# release-notes entry + this constant — #262.
CLAUDE_CODE_PLUGIN_FLOOR="2.1.141"

# Allow `--version vX.Y.Z` as a positional flag for parity with cargo-dist's
# generated installer.
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#--version=}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  linux-x86_64)   TARGET=x86_64-unknown-linux-musl ;;
  linux-aarch64)  TARGET=aarch64-unknown-linux-musl ;;
  linux-arm64)    TARGET=aarch64-unknown-linux-musl ;;
  darwin-x86_64)  TARGET=x86_64-apple-darwin ;;
  darwin-arm64)   TARGET=aarch64-apple-darwin ;;
  *) echo "unsupported platform: $OS-$ARCH" >&2; exit 1 ;;
esac

if [ "$VERSION" = "latest" ]; then
  release_json="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")"
  # Anchor on the `"tag_name":` field name. A heuristic that grabs
  # the last quoted string on the line breaks on single-line JSON
  # (the last quoted string is usually body content, not the tag).
  if command -v jq >/dev/null 2>&1; then
    VERSION="$(printf '%s' "$release_json" | jq -r '.tag_name')"
  else
    VERSION="$(printf '%s' "$release_json" \
              | sed -nE 's/.*"tag_name": *"([^"]+)".*/\1/p' | head -1)"
  fi
  if [ -z "$VERSION" ] || [ "$VERSION" = "null" ]; then
    echo "could not resolve latest release tag from GitHub API" >&2
    exit 1
  fi
fi

# Pick a sha256 tool once. macOS ships shasum; most Linux distros ship sha256sum.
SHA_CMD=""
if [ "$VERIFY" != "0" ]; then
  if command -v sha256sum >/dev/null 2>&1; then
    SHA_CMD="sha256sum"
  elif command -v shasum >/dev/null 2>&1; then
    SHA_CMD="shasum -a 256"
  else
    echo "no sha256 tool on PATH (looked for sha256sum, shasum)" >&2
    echo "set TEAMCTL_VERIFY=0 to skip verification (not recommended)" >&2
    exit 1
  fi
fi

mkdir -p "$INSTALL_DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "installing teamctl $VERSION ($TARGET) -> $INSTALL_DIR"

# cargo-dist publishes one tarball per crate; each contains a single binary
# plus README/LICENSE. Pull all four.
installed=0
for bin in teamctl team-mcp team-bot teamctl-ui; do
  tarball="$bin-$TARGET.tar.xz"
  url="https://github.com/$REPO/releases/download/$VERSION/$tarball"
  sums_url="$url.sha256"

  echo "  fetching $tarball"
  curl -fsSL "$url" -o "$tmp/$tarball"

  if [ "$VERIFY" != "0" ]; then
    if curl -fsSL "$sums_url" -o "$tmp/$tarball.sha256" 2>/dev/null; then
      expected="$(awk '{print $1}' "$tmp/$tarball.sha256")"
      actual="$(cd "$tmp" && $SHA_CMD "$tarball" | awk '{print $1}')"
      if [ "$expected" != "$actual" ]; then
        echo "sha256 mismatch for $tarball" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        exit 1
      fi
    else
      echo "sha256 sidecar not found at $sums_url" >&2
      echo "release may pre-date checksum publishing; set TEAMCTL_VERIFY=0 to skip" >&2
      exit 1
    fi
  fi

  tar -xJf "$tmp/$tarball" -C "$tmp"
  src="$tmp/$bin-$TARGET/$bin"
  if [ ! -x "$src" ]; then
    echo "tarball $tarball did not contain expected binary $bin" >&2
    exit 1
  fi
  install -m 0755 "$src" "$INSTALL_DIR/$bin"
  installed=$((installed + 1))
done

if [ "$VERIFY" != "0" ]; then
  echo "all sha256s verified."
fi
echo "installed $installed binaries to $INSTALL_DIR."

# T-099: Claude Code plugin install/update flow.
# - claude not on PATH               → silent skip
# - claude version < FLOOR (#262)    → skip with remediation message
#                                      (binaries above stay installed)
# - claude --version unparseable     → skip with one-line note
# - plugin already installed         → silent update (best-effort; warns on
#                                      failure but does not fail the install
#                                      since the binaries are already in place)
# - plugin not installed + TTY       → one-time prompt; `y` runs marketplace
#                                      add (skip-if-already-added) + install
# - plugin not installed + non-TTY   → silent skip
# Detection avoids a `jq` dependency by grepping the documented JSON schema
# (`"id":"teamctl@..."` from `claude plugin list --json`).
plugin_installed() {
  claude plugin list --json 2>/dev/null \
    | grep -q '"id"[[:space:]]*:[[:space:]]*"teamctl@'
}

marketplace_present() {
  claude plugin marketplace list --json 2>/dev/null \
    | grep -q '"name"[[:space:]]*:[[:space:]]*"teamctl"'
}

# Parse the first whitespace token of `claude --version`. Documented
# output shape: `<semver> (Claude Code)` (e.g. `2.1.141 (Claude Code)`).
# Empty on parse failure or non-zero exit.
claude_installed_version() {
  claude --version 2>/dev/null | awk '{print $1; exit}'
}

# True (exit 0) iff $1 is a semver >= $2, using version-aware sort
# (handles 2.1.10 > 2.1.9 correctly, unlike string compare). Equality
# returns true. Returns false (non-zero) on unparseable input — caller
# guards against empty $1 separately so the skip message can name the
# installed version.
version_ge() {
  printf '%s\n%s\n' "$2" "$1" | sort -V -C 2>/dev/null
}

if command -v claude >/dev/null 2>&1; then
  installed_claude_version="$(claude_installed_version)"
  if [ -z "$installed_claude_version" ]; then
    # claude is on PATH but `--version` didn't produce a parseable
    # semver. Skip plugin install silently with a one-line note —
    # binary install above is already done.
    echo "note: could not read 'claude --version' output; skipping plugin install" >&2
  elif ! version_ge "$installed_claude_version" "$CLAUDE_CODE_PLUGIN_FLOOR"; then
    # Floor-gated skip (#262). Print the verbatim remediation message
    # from the issue; fall through to the PATH check below — binaries
    # are installed, only the plugin step is gated.
    echo "teamctl Claude Code plugin requires Claude Code >= $CLAUDE_CODE_PLUGIN_FLOOR (installed: $installed_claude_version)."
    echo "Please update Claude Code first:"
    echo "    claude --upgrade   (or your platform's update path)"
    echo "Then re-run the teamctl installer:"
    echo "    curl -fsSL https://teamctl.run/install | sh"
  elif plugin_installed; then
    # Best-effort update path. Suppress noise on success; warn on failure but
    # don't kill the script — the binaries above are the install's main job.
    # `plugin marketplace update` takes the bare marketplace name
    # (`teamctl`); `plugin update` takes the marketplace-qualified
    # plugin id (`teamctl@teamctl`). They are NOT the same argument —
    # do not "sync" these two to the same string. The bare form on
    # `plugin update` always fails with `Plugin "teamctl" not found`,
    # which made this best-effort step unconditionally broken (#298).
    if ! claude plugin marketplace update teamctl >/dev/null 2>&1; then
      echo "note: 'claude plugin marketplace update teamctl' failed (non-fatal; binaries installed OK)" >&2
    fi
    if ! claude plugin update teamctl@teamctl --scope user >/dev/null 2>&1; then
      echo "note: 'claude plugin update teamctl@teamctl' failed (non-fatal; binaries installed OK)" >&2
    fi
  elif [ -t 1 ] && [ -r /dev/tty ]; then
    printf "install teamctl Claude Code plugin? [y/N] "
    if read -r REPLY < /dev/tty; then
      case "$REPLY" in
        y|Y|yes|YES|Yes)
          # Skip the marketplace-add when it's already registered — avoids the
          # error path on a re-run.
          if ! marketplace_present; then
            claude plugin marketplace add Alireza29675/teamctl --scope user
          fi
          claude plugin install teamctl@teamctl --scope user
          ;;
        *)
          echo "skipping plugin install. Re-run later with:"
          echo "  claude plugin marketplace add Alireza29675/teamctl --scope user"
          echo "  claude plugin install teamctl@teamctl --scope user"
          ;;
      esac
    fi
  fi
fi

# T-070: if INSTALL_DIR isn't on PATH, print an actionable shell-tailored
# one-liner instead of a vague "make sure it's on your PATH." Friendly
# without auto-mutating rc files (per T-062).
case ":${PATH:-}:" in
  *":$INSTALL_DIR:"*)
    # Already on PATH — nothing more to say.
    exit 0
    ;;
esac

# System-wide install dirs (running as root) are typically on PATH already;
# if INSTALL_DIR somehow isn't, keep the message generic rather than nudging
# operator rc files.
if [ "$(id -u 2>/dev/null || echo 1)" = "0" ]; then
  echo "note: $INSTALL_DIR is not on root's PATH — add it to your system profile."
  exit 0
fi

echo
echo "WARNING: $INSTALL_DIR is not on your PATH."
echo

case "${SHELL:-}" in
  */zsh)
    rcfile="$HOME/.zshrc"
    echo "To fix it, run:"
    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $rcfile && source $rcfile"
    echo
    echo "Or add the line manually to $rcfile."
    ;;
  */bash)
    rcfile="$HOME/.bashrc"
    echo "To fix it, run:"
    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $rcfile && source $rcfile"
    echo
    echo "Or add the line manually to $rcfile."
    ;;
  */fish)
    echo "To fix it, run:"
    echo "  fish_add_path $INSTALL_DIR"
    echo
    echo "(persists immediately — no need to source.)"
    ;;
  *)
    rcfile="$HOME/.profile"
    echo "To fix it, run:"
    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $rcfile && . $rcfile"
    echo
    echo "Or add the line manually to $rcfile. Some shells (csh, tcsh)"
    echo "use different syntax — adapt accordingly."
    ;;
esac
