---
title: Interfaces
---

An **interface** connects a human conversation to an agent. The CLI and UI Chat
view use the built-in local interface. Telegram and personal WhatsApp are supplied
messenger transports; community adapters can be executables written in any language.
All use the shared conversation repository and scoped reply routing.

Each conversation binds an adapter instance, a provider peer and one canonical
agent. Inbound events preserve source and participant IDs. Agent replies address
that conversation or its inbound mailbox message; ambiguous destinations reject
instead of choosing a chat. History is retained separately from disposable agent
inboxes, so `teamctl gc` does not erase chat transcripts.

| Interface | Chat | Media | Typing/reactions | Approvals |
|---|---|---|---|---|
| Local CLI/UI | Text | Unsupported | Unsupported | Existing CLI/UI approval commands |
| Telegram | Text | Existing image/file/voice behavior | Supported | Existing Telegram buttons |
| Personal WhatsApp | Text and quoted replies | Unsupported | Unsupported | Use existing CLI/UI commands |
| Community process | Declared capabilities | Declared capabilities | Declared capabilities | Not part of protocol v1 |

Local clients share `local` / `operator` conversations. They can read external
transcripts; replies to those threads are sent from their messenger. Telegram keeps
its per-manager configuration and `teamctl bot setup` workflow. WhatsApp uses
explicit foreground QR pairing and separate private login state.

## Community adapters

Declare `type: process` in the top-level `interfaces` array, with an executable,
literal args, known canonical target and exact sender/peer allowlists. Empty lists
deny all inbound messages. The host supplies a private state directory, supervises
the process and owns authorization, persistence and delivery leases.

Adapters exchange versioned JSON lines over stdin/stdout and never open the
mailbox database. An optional Node SDK provides durable inbound spooling and
outgoing receipts. A provider-success/local-persistence crash can still duplicate a
send; reconnect recovery depends on the provider.

See the [chat and messaging guide](/guides/chat-and-adapters/) for configuration,
CLI/UI usage, pairing, lifecycle and the executable contract. Other declarations
without an implemented process transport remain inert.

## Related

- [HITL](/concepts/hitl/)
- [Telegram setup](/guides/telegram-bot/)
- [Chat and messaging adapters](/guides/chat-and-adapters/)
