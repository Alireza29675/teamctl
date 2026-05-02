---
title: Two projects, one teamctl, with bridges
description: Isolated-by-default projects sharing one mailbox; bridges open a time-boxed cross-project channel between two managers.
---

A pattern for running unrelated teams under a single teamctl instance.
Projects are **isolated by default** — nothing in `product` can reach
`blog` without an explicit bridge. When the two managers need to talk
about a specific topic for a bounded time, you open a bridge:

```bash
teamctl bridge open \
  --from product:manager \
  --to   blog:editor \
  --topic "share launch event photos" \
  --ttl 120

teamctl bridge list      # see open / expired / closed bridges
teamctl bridge log 1     # replay the transcript of bridge #1
teamctl bridge close 1
```

While the bridge is open, only the two named managers can DM across.
Every message exchanged is recorded with `thread_id = bridge:<id>` for
audit. The pattern shows up in `examples/newsletter-office/` (newsroom
↔ blog handoff) and `examples/oss-maintainer/` carries the same
HITL spirit at single-project scope (release_manager gates
release-critical actions via plan-mode). The compose file below stays
preserved as a reference recipe.

```yaml
# .team/team-compose.yaml
version: 2

broker:
  type: sqlite
  path: state/mailbox.db

supervisor:
  type: tmux
  tmux_prefix: a-

projects:
  - file: projects/product.yaml
  - file: projects/blog.yaml
```

```yaml
# .team/projects/product.yaml
version: 2

project:
  id: product
  name: Product
  cwd: .

channels:
  - name: all
    members: "*"

managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    reports_to_user: true
    can_dm: [dev]
    can_broadcast: [all]

workers:
  dev:
    runtime: claude-code
    reports_to: manager
    can_dm: [manager]
    can_broadcast: [all]
```

```yaml
# .team/projects/blog.yaml
version: 2

project:
  id: blog
  name: Blog
  cwd: .

channels:
  - name: all
    members: "*"

managers:
  editor:
    runtime: claude-code
    model: claude-opus-4-7
    reports_to_user: true
    can_dm: [writer]
    can_broadcast: [all]

workers:
  writer:
    runtime: gemini
    model: gemini-3.0-pro
    reports_to: editor
    can_dm: [editor]
    can_broadcast: [all]
```

See `concepts/bridges/` for how bridges integrate with HITL — every
bridge open / close is auditable, and the `--ttl` is a hard timeout
(no implicit re-opening).
