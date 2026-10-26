---
title: Human-in-the-loop (HITL)
description: Ask for a human decision through the team's mailbox.
---

Agents can call `request_approval` when they need your decision. TeamCTL records the request, surfaces it through Telegram or the host's approval commands, and returns the decision to the agent.

The agent's instructions and runtime permissions determine when it asks. The mailbox does not intercept arbitrary tool calls or act as a universal sandbox.

## Define the approval policy

Write the actions that require approval into your role instructions. The compose schema includes `autonomy`, `hitl.globally_sensitive_actions` and `hitl.auto_approve_windows` as policy fields. Declaring them does not automatically block runtime actions or automatically approve mailbox requests in the current implementation.

The default sensitive-action list is `publish`, `release`, `payment`, `external_email`, `external_api_post`, `merge_to_main`, `dns_change` and `deploy`.

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

This is policy metadata. For an enforced action boundary, configure the runtime's permissions and any appropriate tool or hook controls. See [Multi-runtime teams](/guides/multi-runtime/) for capability differences.

## Request and decide

```text
agent    → request_approval(action, summary, payload?, ttl_seconds?)
team-mcp → insert a pending approval; wait for a decision by default
Telegram or terminal UI → show the request to you
you      → approve or deny
team-mcp → return the outcome to the agent
```

A pending request is not permission to act. Agents should proceed only when the returned outcome permits the action. Requests have a bounded lifetime; delivered requests expire without a decision, while requests that never reached a human surface become `undeliverable`.

Decide through:

- Telegram's inline approval buttons on a configured manager bot.
- The terminal UI's approval surface.
- `teamctl pending`, `teamctl approve <id>` or `teamctl deny <id>` on the host.

Other interface adapters are planned. See [Interfaces](/concepts/interfaces/) for what ships today.

## Related

- [Interfaces](/concepts/interfaces/)
- [Guide: Bridges and HITL](/guides/bridges-and-hitl/)
