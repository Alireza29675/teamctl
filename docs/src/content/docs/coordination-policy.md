---
title: Coordination policy — how teamctl keeps the mailbox sane
---




A real team needs broadcasts. A team that can't say *"hey everyone, we're shipping in 5"* is not a team — it's a bag of pen-pals.

But "everyone can broadcast to everyone" is also how mailboxes turn into spam. Other agent-mailbox projects (notably Dicklesworthstone's `mcp_agent_mail`) chose to **forbid broadcast** rather than design around it. We respect that choice; we made a different one.

This page covers how teamctl ships broadcast safely:

- What can talk to what.
- Where broadcast is bounded.
- What `gc` does and when.
- The trade-offs we accept.

## What can talk to what

Every agent declares its outbound allowlist explicitly. The mailbox enforces these. There is **no implicit "everyone talks to everyone."**

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

- `can_dm` is a **list** of specific recipients. No wildcards by default. If a worker isn't in `can_dm`, the message is rejected by the broker.
- `can_broadcast` is a **list** of specific channels. Channels are declared at the project level, with explicit membership.
- `reports_to` builds the org chart. A worker without a `reports_to` cannot be reached up the chain.

Cross-project DMs are blocked by default. The only legal way for one project's agent to reach another project's agent is via an **explicit, time-limited bridge** between two managers. See [Bridges](/concepts/bridges/) for the full surface.

## Where broadcast is bounded

- **One channel per project, by default.** In the hello-team example, the only channel is `all`. You can declare more — `#leads`, `#engineering`, `#publishing` — but each one is an explicit list of members.
- **Membership is declared, not discovered.** No agent can join a channel by writing to it. The compose file is the source of truth.
- **Managers see broadcasts on their channels. Workers see broadcasts on theirs.** The org chart shapes the broadcast graph, not the agents themselves.
- **`teamctl gc` is the floor on mailbox volume.** Acked messages older than `message_ttl_hours` are removed; the working set stays bounded regardless of how chatty the channel got.

The result is that a "broadcast" in teamctl is not "shout into the void" — it's "speak to a defined room." Today there is **no** per-agent broadcast frequency throttle in the broker — the bounds above (declared membership, declared channels, allowlisted senders) are the only line of defense, plus the mailbox TTL above. A frequency throttle is a reasonable future addition; we'll add it when we see real abuse, not pre-emptively.

## What `gc` does and when

`teamctl gc` collects garbage from the mailbox on a TTL schedule. Two knobs in `team-compose.yaml`:

```yaml
budget:
  message_ttl_hours: 72   # mailbox rows older than this are eligible for gc
```

Run on demand:

```bash
teamctl gc
teamctl gc --project newsroom
```

`gc` removes:

- Acked messages older than `message_ttl_hours`.
- Resolved approval requests older than the same window.
- Closed bridges and their logs after their TTL plus a grace period.

It does **not** remove unacked messages, pending approvals, or live bridges — those are load-bearing state.

The shorter the TTL, the smaller the working set, the lower the per-turn token cost. A TTL of 24 hours is fine for fast-moving teams; 72 hours is the default; longer than a week is rarely useful.

## Why we ship broadcast despite the spam risk

Three reasons:

1. **Real teams broadcast.** A manager updating a `#leads` channel after a status sync is a normal team interaction, not a spam vector. Forbidding it pushes operators back to ad-hoc DMs in fan-out shapes that *do* spam.
2. **The risk is bounded.** Broadcast is allowlisted, channel-scoped, membership-declared. The worst case is "every agent in one channel sees one extra message" — not "the entire mailbox floods."
3. **HITL backstops the high-cost cases.** The actions that *matter* — `publish`, `release`, `deploy`, `external_email` — block on `request_approval` regardless of how many messages flew first. A noisy team can't ship a noisy mistake.

We acknowledge the alternative design (broadcast off by default; only explicit DMs allowed) and respect it. Operators who want that shape can simply omit `can_broadcast` from every agent's declaration. The compose file lets you opt out of broadcast for the entire team.

## What you can change today

- **Tighten `can_dm` and `can_broadcast` per agent.** Start narrow; widen when you observe a real need.
- **Shrink `message_ttl_hours`.** Faster gc → lower context → less spend.
- **Add channels.** A `#leads` channel that workers can read but not write to keeps strategic chatter out of operational DMs.
- **Mark sensitive actions in `hitl.globally_sensitive_actions`.** Anything that crosses an HITL line stops, regardless of how the agent got there.
- **Use `permission_mode: confirm` while a role is new.** Each tool call is approved manually until trust is established.

## What we will not do

- We will not flip broadcast off by default. Real teams use it; operators who want it off can declare it off.
- If we do add a broadcast-frequency throttle later, it will surface visibly — in the agent's pane and in `teamctl inspect` — not silently drop messages.
- We will not auto-summarize the mailbox without an explicit setting. Summarization is a context choice, not a privacy default.

---

**See also:** [Cost & rate limits](/cost/) for how mailbox volume affects per-turn cost. [HITL](/concepts/hitl/) for the approval flow that backstops sensitive actions.
