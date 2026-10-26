# Executable chat protocol v1

An adapter is a trusted operator-installed executable. teamctl starts it with
literal argv in the team root, a minimal environment plus explicit configured
credential mappings, and separate stdin/stdout pipes. Stdout contains only UTF-8
JSON objects, one per newline. Diagnostic output is drained and discarded by the
host; use controlled error codes rather than provider error text.

The supplied host currently requires Unix (macOS/Linux) for lifetime-lock
inspection. The wire contract remains language independent.

Any language can implement this boundary. The [Node SDK](sdk/README.md) supplies
framing, durable spooling, receipts, locking and shutdown; the [echo executable](examples/echo.js)
is a dependency-free example. Adapters do not open SQLite or choose their agent.
The host pins the target from compose and checks the exact sender and peer.

## Handshake and inbound messages

The host sends exactly one initialization before other work:

```json
{"type":"initialize","version":1,"instance_id":"example","state_dir":"/operator/private/example","mode":"run","config":{}}
```

`config` is the declared `options` object. Credentials arrive in explicitly mapped
environment variables. `state_dir` is an absolute private directory outside the
team root and all agent working directories. Preserve account state on ordinary
shutdown. Keep files private and prevent concurrent ownership of a profile.

When the transport can receive and send, emit:

```json
{"type":"ready","version":1,"capabilities":["text","quoted_reply"],"display_name":"Example"}
{"type":"message","external_id":"source:42","peer_id":"chat:7","sender_id":"person:3","text":"Hello"}
```

Ready is the protocol handshake; connection events describe subsequent state.
Capabilities contain at most 32 bounded strings. Content capabilities are `text`,
`image`, `file`, `audio`, `video`, `voice`, `media`, `media_error`, `reaction` and
`typing`. Advertise only supported kinds; `quoted_reply` additionally describes
quote support. Reaction and typing are outgoing controls, and cannot enter agent
inboxes. System, approval and staging kinds are reserved.

Inbound `kind` defaults to `text`, `payload` to null. Optional `title` names the
conversation; optional `reply_to_external_id` preserves a quoted provider ID.
IDs are opaque: preserve them exactly, including a group's peer and its actual
participant as `sender_id`. Use the same source ID across reconnect/replay.

The host persists an authorized event before responding:

```json
{"type":"accepted","peer_id":"chat:7","external_id":"source:42"}
```

Or it rejects an unauthorized/invalid event:

```json
{"type":"rejected","peer_id":"chat:7","external_id":"source:42","code":"forbidden_sender"}
```

Either acknowledgement removes that exact peer/source item from a durable spool.
A persistence failure stops the host without acknowledging; replay the event on
reconnect. Never mark an event consumed merely because stdout accepted bytes.

## Delivery and control messages

The host leases durable outgoing work, one message at a time:

```json
{"type":"send","request_id":"delivery:91","peer_id":"chat:7","message":{"text":"Hello back","kind":"text","payload":null,"reply_to_external_id":"source:42"}}
```

`request_id` remains stable across retries. After provider acceptance, persist a
receipt before emitting success:

```json
{"type":"sent","request_id":"delivery:91","external_id":"source:43"}
```

`external_id` may be null if the provider has no message ID. Repeated requests
with identical input should return the saved receipt; different input under the
same request ID must fail. Use provider idempotency keys where supported. A crash
after provider acceptance but before local receipt persistence can still duplicate
a send. Neither host nor SDK promises exactly-once delivery. The SDK retains known provider
receipts across transient storage errors in the running process and retries
persistence before acknowledgement, without a second provider call.

A request-scoped failure looks like:

```json
{"type":"error","request_id":"delivery:91","code":"disconnected","message":"Transport disconnected","retryable":true}
```

Use controlled constants for `code` and `message`; never copy secrets, message
text or raw provider errors into them. The host displays a generic failure reason.
Retryable work gets bounded backoff, up to ten attempts. Nonretryable errors become
visible failed transcript rows. A response timeout stops the host and retains
queued work; lease expiry recovers work after a host crash.

`reaction` carries `payload.emoji` and `payload.external_message_id` identifying
an authorized inbound event in that conversation. Native Telegram also receives
its numeric `telegram_msg_id`. `typing` carries a Unix-seconds `expires_at`; ignore
expired work and never extend the indicator beyond its expiry. MCP reports
`supported:false` without queueing controls when the adapter lacks the capability.

## Connection, pairing and shutdown

```json
{"type":"connection","state":"disconnected"}
```

States are `starting`, `connecting`, `connected`, `disconnected`,
`pairing_required`, `auth_failed` and `failed`. A connected event resumes delivery
after a completed Ready handshake. Startup/reconnect deadlines are bounded by the
host's configured timeout. A global error without request ID describes transport
failure; terminal errors stop the host.

In explicit foreground `mode:pair`, an adapter may emit
`{"type":"pairing","qr":"EPHEMERAL_SECRET"}`. This is a credential, and is shown
only to the interactive pairing terminal. Run mode must report pairing_required;
never print, log or persist the QR. Pair mode stops after Ready. Identity inspection
uses run mode without forwarding inbound events or claiming outgoing deliveries.

On `{"type":"shutdown"}`, stdin EOF or termination signals, cancel outstanding
operations and close/reap owned sockets/children. The host enforces bounded writes,
reads and process-group cleanup. Leave successful receipts, pending spool and login
state intact.

## Limits and evolution

Frames are at most 256 KiB including the newline. Text is at most 64 KiB,
structured payload JSON 128 KiB, ordinary identities 1 KiB, and error diagnostics
4 KiB. Identity strings cannot be blank, surrounded by whitespace or contain
control characters. Unknown fields, variants, versions and privileged kinds fail
closed; do not add wire fields without a versioned compatibility design.

The Node SDK additionally bounds task/output queues, state capacity and operation
timeouts; see its [delivery guarantees](sdk/README.md#boundary-and-failure-rules).
Custom adapters must provide their own persistence and provider recovery. The
host's tests use fake executables; SDK tests exercise actual subprocess framing,
acknowledgement isolation, receipt replay and termination without messenger accounts.
