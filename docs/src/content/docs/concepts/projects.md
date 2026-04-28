---
title: Projects
---

A **project** is an isolated team of agents with its own channels, roster, and working directory. Messages from one project cannot reach another without an explicit, time-boxed [bridge](./bridges.md).

```
projects:
  - file: projects/fork-vancouver.yaml
  - file: projects/yad.yaml
```

Each project file declares:

- `project.id` / `project.name` / `project.cwd`
- `channels:` — named groups with ACLs
- `managers:` — agents that can DM the human and emit `reply_to_user`
- `workers:` — everyone else

Why isolation matters: running one fleet across many unrelated projects is the whole point of teamctl, and you do not want a marketing agent on project A accidentally DM-ing an engineering agent on project B. Isolation is enforced by `team-mcp` on every tool call — it is not a convention, it is a check.

## Related

- [Channels](./channels.md)
- [Bridges](./bridges.md)
