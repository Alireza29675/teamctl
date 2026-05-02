---
title: Mixing runtimes in one team
description: One manager, two workers, three different CLI runtimes — Claude Code + Codex + Gemini.
---

A pattern for letting one manager dispatch to workers running on
different CLI stacks — Claude Code for orchestration, Codex for deep
backend reasoning, Gemini for million-token research. All three agents
share the same SQLite mailbox and talk through the same MCP tools; the
manager doesn't know — or care — that its workers run on different
runtimes.

The multi-runtime story is now told inside the curated examples
themselves: `startup-team` mixes Claude + Codex; `oss-maintainer` does
the same; and the `multi-runtime` guide in the docs walks through the
moving parts. The compose file below stays preserved as a reference
recipe.

```yaml
# .team/projects/mixed.yaml
version: 2

project:
  id: mixed
  name: Mixed-Runtime
  cwd: .

channels:
  - name: all
    members: "*"

managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    can_dm: [backend, researcher]
    can_broadcast: [all]

workers:
  backend:
    runtime: codex                 # OpenAI Codex CLI
    model: gpt-5-codex
    reports_to: manager
    can_dm: [manager]
    can_broadcast: [all]
  researcher:
    runtime: gemini                # Google Gemini CLI
    model: gemini-3.0-pro
    reports_to: manager
    can_dm: [manager]
    can_broadcast: [all]
```

Install each CLI you want to use before `teamctl up` — it fails fast
with a clear error if a runtime binary is missing on `$PATH`.
