---
title: Per-agent reasoning effort
---

Some agents on a team carry more weight than others. The
`release_manager` deciding what ships, the `playtest_critic` looking
for the design flaw nobody else will admit — those want every cycle
the runtime can spare. The triage worker scanning a hundred new
issues a day does not. The `effort:` field on a per-agent compose
entry lets you tune that knob per role.

## YAML

```yaml
# .team/projects/oss.yaml
managers:
  release_manager:
    runtime: claude-code
    model: claude-opus-4-7
    role_prompt: roles/release_manager.md
    permission_mode: plan
    effort: max          # ← per-agent reasoning effort
```

Five values, lowest to highest:

| Value    | Notes |
|----------|-------|
| `low`    | Quickest. Good for triage, log-watching, mechanical agents. |
| `medium` | Default-ish. Most workers. |
| `high`   | Deeper analysis. Architecture-leaning roles. |
| `xhigh`  | One step short of max. |
| `max`    | Strongest reasoning. Reserve for plan-mode dissenters and release-critical roles. |

`teamctl validate` rejects unknown values at parse time — `effort: hgih`
fails immediately with the valid set enumerated, rather than silently
falling back to a wrapper default.

## Precedence

When more than one source sets an effort, the highest-priority one wins:

1. Per-agent YAML — `effort:` in the agent's compose entry.
2. Workspace `.env` — `EFFORT=` exported into the team's environment.
3. Unset — the runtime's own default.

In practice that means: a workspace-wide floor in `.env` is overridden
by any agent that explicitly sets its own value. Agents that don't set
`effort:` inherit the workspace setting; agents that don't, and aren't
under a workspace setting, run on the runtime's default.

## When to set it

Reach for `effort:` when:

- One agent on the team carries the *deciding* output and the rest
  feed into it (release planners, lead editors, plan-mode critics).
- A team mixes deep thinking with bulk throughput, and one effort
  setting for everyone would either over-spend on routine work or
  under-spend on the critical role.

Skip it when the team is uniform — a workspace `.env` setting reads
cleaner than five identical YAML lines.

## See also

- [`oss-maintainer` example](https://github.com/Alireza29675/teamctl/tree/main/examples/oss-maintainer) — `release_manager` runs at `effort: max`.
- [`indie-game-studio` example](https://github.com/Alireza29675/teamctl/tree/main/examples/indie-game-studio) — pattern fits its `playtest_critic` role too.
