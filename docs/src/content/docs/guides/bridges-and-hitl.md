---
title: Bridges and HITL
---

This guide covers the two mechanisms that keep teamctl safe: **project bridges** (for cross-project manager collaboration) and **human-in-the-loop approvals** (for brand-sensitive actions).

## Bridges

See [Concepts · Bridges](/concepts/bridges/) for the full reference. TL;DR:

```bash
teamctl bridge open \
  --from newsroom:head_editor \
  --to blog-site:manager \
  --topic "hand off morning brief" \
  --ttl 60

teamctl bridge list
teamctl bridge log 1
teamctl bridge close 1
```

Only the two named managers can DM across while the bridge is open. Every message carries a `thread_id` of `bridge:<id>` for audit.

## HITL

See [Concepts · HITL](/concepts/hitl/). The agent calls `request_approval` before acting:

```
agent → request_approval({ action: "publish", summary: "Post morning brief",
                           payload: { url: "https://blog/..." },
                           ttl_seconds: 900 })
team-mcp → inserts pending row, blocks caller
interface adapter → surfaces to you (Telegram, CLI, …)
you → /approve 42 (or tap Approve button)
agent ← { status: "approved" }, proceeds with real tool call
```

Defaults deny. A missing `decided_at` by `expires_at` auto-transitions to `expired`.

## Patterns

### Nightly publish window

Give yourself a two-hour sleep gap during which a specific scope is pre-approved:

```yaml
hitl:
  auto_approve_windows:
    - action: publish
      project: newsroom
      scope: "morning-brief-*"
      until: "2026-05-01T09:00:00Z"
```

### Manager-only bridge

Workers can't open bridges — only Alireza can, via `teamctl` or Telegram. This is an intentional privilege boundary.

### Audit after the fact

Every bridge has a permanent transcript, and every approval keeps the full `payload_json`. Pull them from SQL or `teamctl bridge log` / the `approvals` table directly for post-hoc review.
