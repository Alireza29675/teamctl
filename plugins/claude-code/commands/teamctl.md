---
description: Open-ended "talk to it" command for evolving an existing teamctl team — add a manager, add a worker, scope a channel, wire telegram, retire an agent.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl` is the ongoing org-evolution command. The user invokes it and describes the change in plain English; the command proposes the YAML diff and applies it on confirmation.

Five v1 verbs:

1. **Add a manager** — with or without telegram.
2. **Add a worker** — reporting to an existing manager.
3. **Scope a channel** — adjust members and `can_broadcast`.
4. **Wire telegram** — on an existing manager.
5. **Retire an agent** — remove from YAML, ACLs cleaned up.

Flow for any change:

1. Read `.team/team-compose.yaml` and `projects/<id>.yaml`.
2. Propose the diff in plain English **and** as a YAML diff.
3. On confirmation, edit the YAML (byte-for-byte hand-authored shape, comments preserved where possible) and run `teamctl validate`.
4. Offer `teamctl reload` to apply.

Read [RULES.md](../RULES.md) before each invocation. Substrate constraint #4 is the non-negotiable: every action this command takes is reproducible with `vim .team/team-compose.yaml`. No skill-only state, no plugin-only formats.

> This is the T-077-A skeleton stub. The real implementation lands in T-077-E. See `.team/tasks/2026-05-03-teamctl-cc-plugin/SLICING.md`.
