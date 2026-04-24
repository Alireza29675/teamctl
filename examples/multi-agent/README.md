# Example: multi-agent

Four agents in one project — `manager`, two `dev`s, one `critic` — across
three channels with different membership and ACLs.

- `#product` — the main channel; all four subscribe.
- `#internal` — `dev1` + `dev2` only; manager and critic don't see these messages.
- `#all` — wildcard; everyone.

Critic runs in `permission_mode: plan` (read-only) to model a reviewer that
cannot mutate anything.

```bash
teamctl validate
teamctl up
teamctl status
```
