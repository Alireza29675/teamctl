---
title: Interfaces
---

An interface is a named human connection to an agent. Provider identifies the technology, such as web or Telegram; instance ID identifies one particular connection. Each instance also has a name, description, capabilities and agent grants.

Two Telegram bots are two interfaces even when they reach the same agent. Agents discover available surfaces with `list_interfaces`, inherit a reply conversation or choose an explicit granted interface. Messages retain source metadata so private/work contexts remain distinguishable.

Web is automatic for every defined agent. Conversation history is durable. Retry IDs prevent repeated provider updates becoming repeated tasks. Revocation blocks new sending and delayed delivery. Historical metadata stays unchanged when an interface is renamed or removed.

See the [browser workspace](../../guides/web-workspace/), [Telegram setup](../../guides/telegram-bot/) and [schema](../../reference/team-compose-yaml/).
