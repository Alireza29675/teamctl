---
title: Migrate from flat layout to `.team/`
---

`0.4.0` made `.team/` the canonical home for a team's compose, role
files, and project files. If your project has been running on
`0.3.x` with a flat layout — `team-compose.yaml`, `roles/`, and
`projects/` sitting at the project root — here is how to move into
the new shape without losing state.

The migration is mechanical: move four things into `.team/`, then
adjust how you invoke `teamctl` from inside the project. No YAML
content changes are required.

## The recipe

Run from the project root, where `team-compose.yaml` currently sits:

```bash
mkdir -p .team
git mv team-compose.yaml .team/
git mv roles .team/                  # if the directory exists
git mv projects .team/               # if the directory exists
echo "state/" >> .team/.gitignore    # or merge into your existing .gitignore
```

`git mv` keeps history readable. `state/` belongs in `.gitignore`
because the mailbox and per-agent runtime files live there and never
leave the host.

## What stays the same

- The YAML schema. `team-compose.yaml`, `projects/<name>.yaml`, and
  any role markdown files keep their existing content; the move is
  pure relocation.
- Agent identities. Each agent's `tmux` session, mailbox messages,
  and per-agent state survive the move because they're keyed on
  `project_id` and `agent name`, not on the file's path.

## What changes

How you invoke `teamctl`:

| Old (flat layout)                           | New (`.team/`)                                            |
|---------------------------------------------|-----------------------------------------------------------|
| `cd <project> && teamctl up`                | `cd <project> && teamctl up` *(walk-up finds `.team/`)*   |
| `teamctl --root .`                          | `teamctl -C <path/to/project>` *(or `cd` and omit `-C`)*  |
| `teamctl context use <path>`                | put `.team/` in `<path>` and `cd` there *(or `-C`)*       |

The `cd <project> && teamctl up` form looks unchanged because
`teamctl` walks up from the current directory to find `.team/`. The
practical difference is that the team config no longer clutters the
project root.

## `teamctl context` deprecation

`teamctl context use <path>` still works in `0.4.0` and prints a
stderr deprecation note pointing at the walk-up convention. The
command is removed in `0.5.0`. The replacement pattern is the same
shape as the migration above: put a `.team/` inside the project and
let walk-up (or `-C`) handle discovery.

## Verify

From the new layout:

```bash
teamctl ps
```

Should show the same agents and the same `running` / `stopped`
status the flat layout did. If a previously-running team was up
during the move, `teamctl down && teamctl up` cleanly restarts the
tmux sessions against the new path.

## See also

- [`oss-maintainer` example](https://github.com/Alireza29675/teamctl/tree/main/examples/oss-maintainer) — ships in the canonical layout.
