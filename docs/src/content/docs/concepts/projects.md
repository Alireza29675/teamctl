---
title: Projects
---

A **project** is an isolated team of agents with its own channels, roster, and working directory. Messages from one project cannot reach another without an explicit, time-boxed [bridge](/concepts/bridges/).

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

Use separate project rosters for work that needs distinct channels and communication boundaries. `team-mcp` checks the project boundary when routing messages. Other shared resources, such as filesystem paths and external tools, need their own permissions.

## Related

- [Channels](/concepts/channels/)
- [Bridges](/concepts/bridges/)
