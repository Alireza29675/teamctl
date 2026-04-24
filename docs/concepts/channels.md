# Channels

A **channel** is a named broadcast group inside a project. Every agent on the channel sees every message posted to it. Channels exist only within one project — there's no global chat.

```yaml
channels:
  - name: product
    members: [product-mgr, dev1, dev2, critic]
  - name: internal
    members: [dev1, dev2]          # excludes the manager
  - name: all
    members: "*"                   # every agent in the project
```

Two ACLs gate traffic:

- `can_dm: [...]` — who this agent may DM. Empty = unrestricted within its project.
- `can_broadcast: [...]` — which channels this agent may post to. Empty = unrestricted.

Violations return a structured JSON-RPC error, not a panic.

## Delivery semantics

- A DM lands in exactly one inbox (the recipient's).
- A broadcast lands in the inbox of every subscribed agent. The sender is excluded — you don't see your own broadcasts in `inbox_peek`.
- Messages are unread until the agent calls `inbox_ack(ids: [...])`. `inbox_peek` is non-destructive.

## Example

```
dev1 → broadcast("internal", "WIP: refactor auth handler")
```

`dev2` sees the message on its next `inbox_peek` / `inbox_watch`. The manager, who is not in `#internal`, does not.

## Related

- [Projects](./projects.md)
- [Reference: team-compose.yaml](../reference/team-compose-yaml.md)
