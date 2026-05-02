---
title: Multi-agent ACLs in one project
description: Four agents in one project, three channels, plan-mode critic.
---

A pattern for fitting more than one worker (and a read-only critic) under
a single manager — with channel membership doing the ACL work, not
runtime gating. `#product` is the open team room; `#internal` is a
back-channel for the two devs only; `#all` is the wildcard. The critic
sits in `permission_mode: plan` and so cannot mutate anything it
reviews.

The patterns here also show up organically in `examples/startup-team/`
(layered managers + workers) and `examples/oss-maintainer/`
(channel-isolated pipeline). The compose file below stays preserved as
a reference recipe.

```yaml
# .team/projects/swarm.yaml
version: 2

project:
  id: swarm
  name: Multi-Agent Swarm
  cwd: .

channels:
  - name: product
    members: [manager, dev1, dev2, critic]
  - name: internal
    members: [dev1, dev2]
  - name: all
    members: "*"

managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    reports_to_user: true
    can_dm: [dev1, dev2, critic]
    can_broadcast: [product, all]

workers:
  dev1:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager, dev2, critic]
    can_broadcast: [product, internal]
  dev2:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager, dev1, critic]
    can_broadcast: [product, internal]
  critic:
    runtime: claude-code
    model: claude-opus-4-7
    permission_mode: plan
    reports_to: manager
    can_dm: [manager, dev1, dev2]
    can_broadcast: [product]
```

The critic in `permission_mode: plan` is the same archetype as
`market-analysts/quant_risk` and `indie-game-studio/playtest_critic` —
a read-only dissenter that proposes but never mutates.
