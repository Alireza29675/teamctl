---
description: First-run teamctl onboarding — from no-teamctl-installed to a running supervised team in one conversation.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl-init` runs the 7-stage onboarding flow:

1. **Detect & install** — probe for tmux, git, claude, teamctl on PATH; propose the right install path if teamctl is missing.
2. **What's the team for?** — offer the four named defaults (OSS maintainer, editorial room, indie studio, solo triage) plus the custom escape hatch.
3. **Propose org** — render the proposed team as a named ASCII tree; confirm before scaffolding.
4. **Init + reveal** — scaffold `.team/` directly to match the canonical `examples/<name>/.team/` shape, generate role prompts on the fly, run `teamctl validate`, then reveal: *"I wrote `.team/team-compose.yaml` for you — open it, everything we just talked about is in there."*
5. **Run** — `teamctl up`, brief tmux status check.
6. **Telegram** — wrap `teamctl bot setup` per user-facing manager; per-manager voice-customization sub-beat.
7. **UI + lifecycle** — three lines on `teamctl ui`, `teamctl reload`, `teamctl down && teamctl up`. Closing: *"you're done. the team is yours."*

Resumable and idempotent — re-running skips stages already done.

Read [RULES.md](../RULES.md) before each stage. Voice rails: 1-2 sentences per beat, "experienced reliable coworker", no walls of text. Substrate constraints are non-negotiable.

> This is the T-077-A skeleton stub. The real flow lands across T-077-B (Stages 1-3), T-077-C (Stage 4), and T-077-D (Stages 5-7). See `.team/tasks/2026-05-03-teamctl-cc-plugin/SLICING.md`.
