#!/usr/bin/env bash
#
# PreToolUse fmt+lint gate — wired on eng-claude for Edit|Write.
#
# Claude Code runs this before every Edit/Write, passing the tool input
# as JSON on stdin. We format the file being written and lint it; if the
# linter finds real problems we exit 2, which blocks the edit and feeds
# the reason back so the engineer fixes it before it ever lands on disk.
#
# Missing dev tools degrade to a warning, not a hard block — the gate
# should sharpen the team, not wedge it on a machine that hasn't run
# `npm install` yet. Swap prettier/eslint for your own project's
# formatter and linter when you repoint this team at your repo.
set -euo pipefail

input="$(cat)"

# Pull the target path out of the tool input. Prefer jq; fall back to a
# portable sed when jq isn't installed.
if command -v jq >/dev/null 2>&1; then
  file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty')"
else
  file="$(printf '%s' "$input" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
fi

# No path, or a file that doesn't exist yet → nothing to format; allow.
[ -n "${file:-}" ] || exit 0
[ -f "$file" ] || exit 0

case "$file" in
  *.js | *.jsx | *.ts | *.tsx | *.css | *.html | *.json | *.md)
    if ! command -v npx >/dev/null 2>&1; then
      echo "fmt-lint: npx not found; install Node to enable the fmt+lint gate." >&2
      exit 0
    fi
    # Format in place. A missing formatter is a warning, not a block.
    npx --no-install prettier --write "$file" >/dev/null 2>&1 ||
      echo "fmt-lint: prettier unavailable on $file (skipping format)." >&2
    # Lint the scriptable files. Only gate when eslint is actually
    # installed — a missing linter warns rather than blocks, so a fresh
    # checkout that hasn't run `npm install` is never wedged. We probe
    # for eslint first because `npx --no-install eslint` exits non-zero
    # both when the package is missing AND when it finds lint problems;
    # without the probe we couldn't tell "no linter" from "real failure".
    case "$file" in
      *.js | *.jsx | *.ts | *.tsx)
        if npx --no-install eslint --version >/dev/null 2>&1; then
          if ! npx --no-install eslint "$file" >&2; then
            echo "fmt-lint: eslint reported problems in $file — fix them before this edit lands." >&2
            exit 2
          fi
        else
          echo "fmt-lint: eslint not installed; skipping lint gate (run 'npm install' to enable)." >&2
        fi
        ;;
    esac
    ;;
esac

exit 0
