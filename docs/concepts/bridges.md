# Bridges

Projects are isolated by default. A **bridge** is a time-boxed authorization you open between two managers in different projects so they can DM each other for a specific topic.

```bash
teamctl bridge open \
  --from fork-vancouver:marketing-mgr \
  --to   yad:product-mgr \
  --topic "share event-photo pipeline" \
  --ttl 2h
```

While open:

- Only the two named managers can cross.
- Every message records `bridge_id` and is visible in `teamctl bridge log <id>`.
- Other agents in either project stay isolated.

On TTL expiry (or `teamctl bridge close <id>`) further cross-project DMs are rejected.

## Related

- [Projects](./projects.md)
- [Guide: Bridges and HITL](../guides/bridges-and-hitl.md)
