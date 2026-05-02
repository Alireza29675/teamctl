---
title: Updating teamctl
---

`teamctl update` re-runs whichever installer brought teamctl in. It
detects the install method automatically, checks GitHub for the
latest release, and either no-ops (already on latest) or executes
the matching update command.

```bash
teamctl update                # detect, prompt, update
teamctl update --check        # only print version comparison
teamctl update --yes          # skip the "Proceed?" confirmation
teamctl update --method brew  # override autodetect
```

## What it runs

| Detection                                           | Update command                                  |
|-----------------------------------------------------|-------------------------------------------------|
| Path contains `Cellar/teamctl/` or `linuxbrew`      | `brew update && brew upgrade teamctl`           |
| Path under `~/.cargo/bin/`                          | `cargo install teamctl team-mcp team-bot --force` |
| Anything else (default — shell installer)           | `curl -fsSL https://teamctl.run/install \| sh`  |

The command then surfaces the installer's own output (download
progress, sha256 verification, brew formula resolution) so you see
exactly what's happening.

## Flags

- `--check` — print the version comparison and exit. Useful in CI or
  in a `crontab` line that just nags you when an update is available.
- `--yes` / `-y` — skip the `Proceed? [Y/n]` prompt. Required if the
  command is being driven by another script.
- `--method <shell|brew|cargo>` — override autodetect. Useful when:
  - You installed via the shell installer but want to switch to brew
    (run `teamctl update --method brew` once and the next autodetect
    will pick up the new path).
  - The autodetect is wrong (rare; please file an issue with your
    `which teamctl` output).

## What it doesn't do

- **No background updates.** Updates only happen when you ask.
- **No partial-state cleanup.** If the new version changes schema or
  paths, follow the CHANGELOG migration notes — `teamctl update` runs
  the installer, nothing more.
- **No PATH modifications.** The shell installer prints PATH advice
  when its target dir isn't on `$PATH`; `teamctl update` doesn't
  rewrite shell rc files.
