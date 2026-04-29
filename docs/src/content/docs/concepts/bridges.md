---
title: Bridges
---

Projects are isolated by default. A **bridge** is a time-boxed authorization you open between two managers in different projects so they can DM each other.

```bash
teamctl bridge open \
  --from product:manager \
  --to blog:editor \
  --topic "share launch event photos" \
  --ttl 120
```

While the bridge is open:

- Only the two named managers (not workers) can DM across.
- Every message they exchange is logged with `thread_id = "bridge:<id>"` for audit.
- Other agents in either project stay isolated.

On TTL expiry (or `teamctl bridge close <id>`) further cross-project DMs are rejected with a `project isolation` error.

## Why not just DM freely?

Without enforced isolation, a marketing agent in Project A could accidentally (or, worse, adversarially) leak intent or data to an engineering agent in Project B. Bridges make cross-talk explicit, time-bounded, and auditable.

## Commands

```bash
teamctl bridge open --from <proj>:<mgr> --to <proj>:<mgr> --topic "..." --ttl <minutes>
teamctl bridge close <id>
teamctl bridge list       # id, endpoints, state (open/expired/closed), topic
teamctl bridge log <id>   # full transcript
```

## Related

- [Projects](/concepts/projects/)
- [Guide: Bridges and HITL](/guides/bridges-and-hitl/)
