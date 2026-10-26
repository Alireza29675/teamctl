---
title: Coordination policy
description: Define who can talk, where conversations belong and how to maintain the mailbox.
---

TeamCTL connects agents through direct messages and project-scoped channels. Choose the communication shape that fits your team: a peer conversation, a shared project room or separate channels for different responsibilities.

## Define outbound permissions

```yaml
managers:
  manager:
    can_dm: [dev]            # one specific worker
    can_broadcast: [all]     # one specific channel

workers:
  dev:
    reports_to: manager
    can_dm: [manager]        # back to the manager
    can_broadcast: [all]
```

A non-empty `can_dm` list restricts direct-message recipients. A non-empty `can_broadcast` list restricts the channels an agent may post to. Empty or omitted lists are unrestricted within the project; omitting a list does not deny that kind of traffic.

Broadcasting also requires membership in the named channel. Membership is declared in configuration, not discovered by agents. A `reports_to` link records structure; it does not grant or restrict communication by itself.

Cross-project messages are blocked unless an active [bridge](/concepts/bridges/) connects the two named managers. That mailbox boundary applies even when their outbound lists are empty.

## Give conversations a home

Use a channel when several agents need the same information. Separate channels can keep research, implementation and review conversations focused.

Every agent on a channel receives its broadcasts, apart from the sender. There is no broker-level per-agent frequency throttle today. Role instructions should describe when to send an update, how much detail to include and how to avoid repetitive exchanges.

Mailbox isolation governs messages. Agents with shared filesystem or external-tool access may have other ways to exchange data; configure those permissions separately.

## Maintain the mailbox

The global compose file sets the age at which acknowledged messages are eligible for garbage collection:

```yaml
budget:
  message_ttl_hours: 72   # acknowledged rows older than this are eligible for gc
```

Run collection on the host:

```bash
teamctl gc
```

It removes acknowledged messages older than the configured horizon and marks overdue pending approval requests as `expired` or `undeliverable`, depending on whether they reached a human surface. The default horizon is 24 hours.

Unread messages are retained. A TTL is not an unread-backlog limit, a rate throttle or a provider-spending cap. Inspect persistent inbox growth with `teamctl mail` and review why agents are not acknowledging their messages.

## Keep action boundaries explicit

Instruct agents to call `request_approval` for actions that need your decision. TeamCTL records those requests and returns the outcome; the sensitive-action list does not automatically intercept arbitrary runtime tools.

Configure runtime permissions and tool access to match the role's scope. A quiet mailbox does not guarantee correct work, and a communication allowlist does not sandbox the agent's filesystem or external actions.

## Adjust from observed work

- Restrict non-empty outbound lists to recipients and channels the agent needs.
- Define channel membership around the information each role should receive.
- Keep messages concise and include the evidence needed for the next step.
- Inspect the sessions, inboxes and approval requests before widening the team.

See [Channels](/concepts/channels/) for delivery semantics, [HITL](/concepts/hitl/) for decisions, and [Cost and rate limits](/cost/) for provider usage.
