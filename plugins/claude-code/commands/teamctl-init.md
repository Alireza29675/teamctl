---
description: First-run teamctl onboarding — from no-teamctl-installed to a running supervised team in one conversation.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl-init` is the first-run onboarding for teamctl. Seven stages: prerequisites and install (Stage 1), pick a team shape (Stage 2), confirm the proposed org (Stage 3), scaffold `.team/` and reveal the YAML (Stage 4), bring it up (Stage 5), wire Telegram (Stage 6), point at the lifecycle commands (Stage 7).

Read [RULES.md](../RULES.md) before each stage. Voice rails: 1-2 sentences per beat, "experienced reliable coworker", emojis sparingly. Body voice is runtime-neutral. *"Claude Code runtime"* is a fact about the agent and stays; *"Claude reads the file"* is voice drift and goes. Substrate constraints are non-negotiable. The flow is resumable and idempotent — re-running skips anything already done.

## Stage 1 — Detect & install

Probe for prerequisites in this order: `tmux`, `git`, `claude`, `teamctl`. Use `command -v` (or `which`) under `Bash`, one probe per tool. Report inline as a tight bullet:

```
✓ tmux        ✓ git        ✓ claude        ✗ teamctl
```

If all four check out, the prereq line plus one beat moves to Stage 2:

> All four are in place. Ready to set up your team?

No celebration prose, no walls. If `teamctl` is missing, choose an install path by autodetect:

- **macOS with `brew` on PATH**: propose `brew install teamctl`. Confirm before running.
- **Linux, WSL, or macOS without brew**: propose `curl -fsSL https://teamctl.run/install | sh`. Confirm before running.
- **If brew or the curl installer doesn't fit** (sandboxed shell, locked-down corp env, build-from-source preference), use the cargo fallback verbatim:

  > Brew and the curl installer don't fit here. Building from source is the path: `cargo install teamctl team-mcp team-bot` if you've got Rust; otherwise install `rustup` first (https://rustup.rs).

Run the chosen command yourself when the user confirms and the harness allows it; otherwise hand the user the exact line to paste. Either way, verify with `teamctl --version` after install and report the version inline. If the version probe fails, name the error in one line and offer to retry or switch install path — don't restart the stage.

If `tmux`, `git`, or `claude` are the ones missing, name what's missing and the canonical install path for the user's platform (`brew install tmux`, the Claude Code installer, etc.). Don't pretend to install runtimes the plugin can't reasonably manage — surface the gap and pause.

## Stage 2 — What's the team for?

Single beat:

> What kind of work? Pick one or describe yours:

Then the four named defaults, **verbatim**, in this order:

> 1. OSS maintainer — a maintainer + 4 workers (triage, bug-fix, docs, release-manager); pauses for you on releases and merges to main.
> 2. Editorial room — an editor + 3 workers (writer, fact-checker, seo-research); pauses for you before publish.
> 3. Indie studio — a director + 3 workers (designer, writer, playtest-critic); pauses for you before releases or outbound emails.
> 4. Solo triage — a manager + 2 workers (research, inbox); pauses for you before sending anything external.

Then the escape hatch, **verbatim**:

> Or: tell me what your team should look like and I'll scaffold one to fit.

If the user picks 1, 2, 3, or 4, advance to Stage 3 with that named default. The pick is sticky — re-running the command later resumes from this point.

If the user picks the escape hatch and describes a custom team, hold for v1 with this surface:

> v1 ships with the four named defaults; describing your own team in plain English is on the way. Pick one of the four for now and you can edit afterwards with `/teamctl`.

Then reoffer the four picks. One graceful surface, no apology spiral.

## Stage 3 — Propose org

Render the chosen default's org as a **named** ASCII tree. The team is named — never `team-1`, never `default`. Infer the name from the cwd's directory:

- `~/dev/acme-blog` → `Acme blog`
- `~/projects/sidequest-game` → `Sidequest game`

If the cwd name is generic (`workspace`, `project`, `dev`, `code`, `src`, single letter), prompt once with a sensible default for the chosen team type:

> What should I call this team? (`OSS maintainers` / `Editorial desk` / `Studio team` / `Triage desk`)

If the user just hits enter, take the offered default.

The tree maps the chosen default's actual roster from `examples/<name>/.team/projects/<id>.yaml`. Manager on top, "← you talk to this one on Telegram" annotation on the manager box, workers fanning out below.

### OSS maintainer (5 agents)

```
                ┌──────────────┐
                │  maintainer  │ ← you talk to this one on Telegram
                └──────┬───────┘
                       │
       ┌───────────┬───┴────┬─────────────────┐
       │           │        │                 │
  ┌────▼─────┐ ┌───▼────┐ ┌─▼────┐ ┌──────────▼────────┐
  │  triage  │ │bug_fix │ │ docs │ │  release_manager  │
  └──────────┘ └────────┘ └──────┘ └───────────────────┘
```

### Editorial room (4 agents)

```
              ┌──────────────┐
              │ head_editor  │ ← you talk to this one on Telegram
              └──────┬───────┘
                     │
       ┌─────────────┼──────────────────┐
       │             │                  │
  ┌────▼───────┐ ┌───▼──────────┐ ┌─────▼────────┐
  │news_writer │ │ fact_checker │ │ seo_research │
  └────────────┘ └──────────────┘ └──────────────┘
```

### Indie studio (4 agents)

```
              ┌──────────────┐
              │   director   │ ← you talk to this one on Telegram
              └──────┬───────┘
                     │
       ┌─────────────┼──────────────────┐
       │             │                  │
  ┌────▼─────┐ ┌─────▼────┐ ┌───────────▼───────┐
  │ designer │ │  writer  │ │  playtest_critic  │
  └──────────┘ └──────────┘ └───────────────────┘
```

### Solo triage (3 agents)

```
       ┌──────────────┐
       │   manager    │ ← you talk to this one on Telegram
       └──────┬───────┘
              │
       ┌──────┴───────┐
       │              │
  ┌────▼─────┐  ┌─────▼────┐
  │ research │  │  inbox   │
  └──────────┘  └──────────┘
```

Closing line, with the agent count for the chosen default:

```
N Claude Code agents · Opus 4.7 · effort high. ship it?
```

Where N is `5` for OSS maintainer, `4` for editorial room, `4` for indie studio, `3` for solo triage. The runtime descriptor is intentional — the v1 plugin scaffolds Claude Code agents on Opus 4.7 at high effort, per the parent ticket. Sister plugins handle other runtimes.

If the user confirms with "ship it", "yes", "go", or similar, advance to Stage 4. If they push back — wanting to drop a worker, swap a model, route a manager through Slack instead of Telegram — surface this once:

> v1 ships the four named defaults as-is; the `/teamctl` ongoing skill (after init) handles edits. Want to ship as-is and I'll point you at `/teamctl` afterwards?

Take a yes/no. If yes, advance. If no, accept it gracefully and exit; the user can re-run `/teamctl-init` later or hand-author `.team/team-compose.yaml` directly.

## Stages 4-7 — handed off

Stage 4 (init + reveal), Stages 5-7 (run / Telegram / lifecycle) live in T-077-C and T-077-D. The handoff point is the user saying "ship it" at the end of Stage 3; Stage 4 picks up from there with the `.team/` scaffolder.

Substrate constraints recap, in case any stage tempts a shortcut:

1. The plugin name on the marketplace card is **`teamctl`** — internal command names stay descriptive (`/teamctl-init`, `/teamctl`).
2. The reveal beat ("I wrote `.team/team-compose.yaml` for you…") fires at the end of Stage 4. Don't pre-empt it here.
3. The `.team/` output Stage 4 produces is byte-for-byte identical to a hand-authored team — no plugin-only state, no generated-by markers.
4. Every action this command takes is reproducible by hand-editing YAML afterwards. Stages 1-3 don't write `.team/`, so this constraint mostly binds Stage 4 onwards, but the prose here doesn't promise anything Stage 4 won't deliver.

> See `.team/tasks/2026-05-03-teamctl-cc-plugin/SLICING.md` for the full slice plan.
