# teamctl — Claude Code plugin

The teamctl onboarding and control surface from inside Claude Code.

Two slash commands ship with this plugin:

- `/teamctl:init` — first-run onboarding. Walks you from no-teamctl-installed to a running supervised team in one conversation. Resumable; skips stages already done.
- `/teamctl:adjust` — open-ended "talk to it" command for evolving the team afterwards. Add an agent session, scope a channel, wire telegram, remove an agent session — described in plain English, applied as YAML edits.

Both commands write to `.team/` in your project. The output is plain YAML and Markdown — yours to read, edit, commit, and run from any agent CLI teamctl supports.

See [RULES.md](./RULES.md) for the guardrails both commands honour, and [role-prompt-style.md](./role-prompt-style.md) for the spine every generated role prompt follows.

> Full README content lands with T-077-D once the onboarding flow is end-to-end. This file is a placeholder for the v1 skeleton.
