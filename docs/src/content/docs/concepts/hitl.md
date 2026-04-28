---
title: Human-in-the-loop (HITL)
---

teamctl agents can act autonomously on low-risk work. Anything **brand-sensitive** pauses and asks you first — over any configured interface (Telegram, Discord, iMessage, CLI, webhook).

## Three layers of safety, outside-in

1. **Per-agent `autonomy`** — `full`, `low_risk_only` (default), or `proposal_only`.
2. **Globally sensitive actions** — a list in global `team-compose.yaml`. Any tool call tagged with one of these is routed through `request_approval`.
3. **Auto-approve windows** — pre-authorize specific actions in a scope for a bounded time.

## Default sensitive actions

`publish`, `release`, `payment`, `external_email`, `external_api_post`, `merge_to_main`, `dns_change`, `deploy`.

Configure in global compose:

```yaml
hitl:
  globally_sensitive_actions:
    - publish
    - release
    - deploy
  auto_approve_windows:
    - action: publish
      project: blog
      scope: "social-media-launch-v3"
      until: "2026-04-25T18:00:00Z"
```

## Agent flow

```
agent    → request_approval(action, summary, payload?, ttl_seconds?)
team-mcp → insert row in `approvals` (status: pending), block caller on long-poll
interface adapter → surface to you on Telegram/Discord/iMessage/CLI
you      → Approve / Deny / Why?
team-mcp → update status, caller unblocks, returns { id, status, note }
```

The default is **deny until explicitly approved**. Requests expire after the agent-supplied `ttl_seconds` (30s–3600s, default 900s).

## Deciding

From any of:

- Inline buttons on the interface that notified you (Telegram/Discord/iMessage).
- `teamctl pending` / `teamctl approve <id>` / `teamctl deny <id>` on the host.
- The `approvals` table directly (scripts, audit tools).

## Related

- [Interfaces](./interfaces.md)
- [Guide: Bridges and HITL](../guides/bridges-and-hitl.md)
