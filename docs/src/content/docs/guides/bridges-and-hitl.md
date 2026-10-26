---
title: Bridges and HITL
---

Use project bridges for a bounded conversation between managers in different projects. Use human-in-the-loop requests when an agent needs your decision before acting. TeamCTL enforces mailbox boundaries; runtime permissions and role instructions govern the agents' other actions.

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

A pending request is not authorization. The agent should continue only after an approving outcome. A delivered request with no decision expires; one that never reached a human surface becomes `undeliverable`.

## Patterns

### Document a time-limited policy

The schema can record an intended pre-authorization window:

```yaml
hitl:
  auto_approve_windows:
    - action: publish
      project: newsroom
      scope: "morning-brief-*"
      until: "2026-05-01T09:00:00Z"
```

In the current implementation, this field does not automatically approve `request_approval` calls or intercept publishing tools. Explicitly configure any enforced controls in the runtime or tools, and describe the agreed scope in the role instructions.

### Manager-only bridge

Only the named managers can exchange messages through the bridge. The operator opens it with `teamctl bridge open`; the agent mailbox has no bridge-opening MCP tool.

### Audit after the fact

Every bridge has a permanent transcript, and every approval keeps the full `payload_json`. Pull them from SQL or `teamctl bridge log` / the `approvals` table directly for post-hoc review.
