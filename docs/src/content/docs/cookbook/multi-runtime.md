---
title: Mixing runtimes in one team
description: One manager, two workers, three different CLI runtimes — Claude Code + Codex + Gemini.
---

One possible TeamCTL configuration: a lead using Claude Code, a builder using Codex and a researcher using Gemini CLI. They share the same mailbox and messaging tools. Choose runtimes and models around the roles and access your work needs.

The [newsletter-office](https://github.com/Alireza29675/teamctl/tree/main/examples/newsletter-office) example mixes Claude Code, Codex and Gemini CLI; [oss-maintainer](https://github.com/Alireza29675/teamctl/tree/main/examples/oss-maintainer) uses Claude Code and Codex. The [multi-runtime guide](/guides/multi-runtime/) explains capability differences. The excerpt below shows one possible roster.

The excerpt shows runtime selection. Role files should define responsibilities and conversations; adapt a complete example when you want those files alongside the roster.

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
    model: claude-opus-4-8
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

Install each CLI you want to use before `teamctl up` — it fails fast with a clear error if a runtime binary is missing on `$PATH`.
