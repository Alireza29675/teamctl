# Channels

A **channel** is a named broadcast group inside a project. Agents subscribed to a channel see every message posted to it. Channels exist only within one project.

```yaml
channels:
  - name: product
    members: [product-mgr, dev1, dev2, critic]
  - name: leads
    members: [product-mgr, marketing-mgr]
  - name: all
    members: "*"          # every agent in the project
```

Two ACLs gate traffic:

- `can_dm: [...]` — who this agent may DM.
- `can_broadcast: [...]` — which channels this agent may post to.

Violations return a structured error; they never panic or silently drop.

## Related

- [Projects](./projects.md)
- [Reference: team-compose.yaml](../reference/team-compose-yaml.md)
