# Human-in-the-loop (HITL)

teamctl agents can take autonomous action on low-risk work. Anything **brand-sensitive** pauses and asks you first — over Telegram, or on the CLI.

Three layers of safety, outside-in:

1. **Per-agent `autonomy`** — `full`, `low_risk_only` (default), or `proposal_only`.
2. **Globally sensitive actions** — a list in global `team-compose.yaml`. Any tool call tagged with one of these is routed through `request_approval`.
3. **Auto-approve windows** — you can pre-authorize specific actions in a scope for a bounded time (`teamctl approve …`).

## Default sensitive actions

- `publish` — external publications (blog, social, forum posts)
- `release` — package releases, tags, deploys
- `payment` — anything that moves money
- `external_email` — email to non-whitelisted recipients
- `external_api_post` — webhooks / POSTs to untrusted hosts
- `merge_to_main` — git merge into `main` / `master`
- `dns_change`
- `deploy`

## Approval flow

```
agent      → request_approval(action, summary, payload)
team-mcp   → check auto_approve_windows → miss
team-mcp   → team-bot → Telegram: "🔐 Approve / Deny / Why?"
agent      ← blocks on long-poll until decided
```

The default for every sensitive action is **deny until explicitly approved**. Failing open is a security bug.

## Related

- [Guide: Bridges and HITL](../guides/bridges-and-hitl.md)
- [Reference: team-compose.yaml](../reference/team-compose-yaml.md)
