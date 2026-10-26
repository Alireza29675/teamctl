# @teamctl/adapter-sdk

A dependency-free Node 22.12+ package for teamctl's executable messaging protocol.
It provides strict JSON-lines framing, durable inbound replay, outgoing receipts,
private state locking and bounded shutdown. The package can be installed from this
workspace or packed for reuse; publication to a registry is a separate action.

## Implement a transport

```js
import { runAdapter, AdapterError } from '@teamctl/adapter-sdk';

process.umask(0o077);
process.exitCode = await runAdapter({
  displayName: 'Example messenger',
  capabilities: ['text', 'quoted_reply'],
  factory: async (initialization, context) => ({
    async start() {
      // Connect using initialization.config and context.signal.
      // Register the provider receive callback, then announce connection readiness.
      await context.ready();
    },
    async send({ request_id, peer_id, message }) {
      // Resolve only after provider acceptance. Return the provider message ID,
      // or null when the provider has no ID. Use request_id for provider idempotency.
      // Throw controlled errors, for example:
      // throw new AdapterError('disconnected', 'Transport disconnected', true);
      return 'provider-message-id';
    },
    async close() {
      // Cancel work and close/reap owned connections or children; preserve login.
    },
  }),
});
```

`factory(initialization, context)` returns a transport with `send` and `close`.
Optional `start` begins asynchronous connection setup. Factories should return
quickly and honor the context AbortSignal. A late factory result is closed after
shutdown. Provider calls execute outside the serialized state queue, so a send
may await an inbound callback without deadlocking.

The context methods are:

| Method | Meaning |
| --- | --- |
| `ready()` | Emit one v1 ready frame and replay pending inbound events in run mode. |
| `receive(event)` | Persist a normalized event before emission; resolves after persistence/output. |
| `connection(state)` | Report starting, connecting, connected, disconnected, pairing_required, auth_failed or failed. |
| `pairing(qr)` | Emit an ephemeral secret only in explicit pair mode; run mode reports pairing_required. |
| `fail(code)` | Emit a controlled terminal error and close the owned transport. |
| `signal` | AbortSignal set when the adapter closes. |

An ordinary inbound event carries `external_id`, `peer_id`, `sender_id` and `text`.
Optional fields are `kind` (default text), `payload` (default null), `title`, and
`reply_to_external_id`. Provider IDs are opaque strings. Preserve both the group
peer and actual participant sender; the host checks both allowlists. Use stable
source IDs across reconnects. `receive` resolves after local durability, while
the host's accepted/rejected frames remove pending spool entries.

Send frames carry a stable `request_id`, peer and normalized message. A successful
receipt is saved before the sent frame is emitted. Replaying the same request
returns that receipt without contacting the provider; reuse with different input
fails. Providers with a native idempotency key should also use the request ID.
If receipt persistence fails after provider success, the running SDK retains that
receipt and retries storage without calling the provider again. New provider sends
wait until the pending receipt becomes durable.
A process crash before durable storage still leaves the duplicate window described
below.

Return an actionable `unsupported_kind` error for capabilities you do not support.
System/control and unknown privileged kinds are rejected by the SDK.

## Boundary and failure rules

Stdout is protocol only. Never log QR strings, credentials, raw provider objects or
message text. Unexpected exceptions become generic errors. `AdapterError` messages
must be developer-controlled constants; never construct them from provider error
text. Diagnostics contain controlled codes only, capped at 128 lines per process.
The supplied WhatsApp entrypoint also suppresses third-party stdout/stderr.

Ready declarations contain at most 32 bounded capability strings. Typing and reactions are outgoing controls; inbound controls cannot enter agent inboxes.

The SDK validates UTF-8 and schema fields, bounds each frame to 256 KiB including
newline, text to 64 KiB, IDs to 1 KiB and structured payloads to 128 KiB. Output has at
most 64 queued frames/2 MiB; state operations have at most 64 queued tasks. Send and
factory operations default to 30 seconds, cleanup to 5 seconds. Stdin EOF,
shutdown, SIGINT and SIGTERM close the transport. The host additionally supervises
the process and its owned process group.

The host supplies an absolute state directory outside the executable's current
workspace. Directories are 0700, files 0600, and state reads reject symlinks,
non-regular files, wrong ownership and permissive file modes. Atomic writes sync
the file and directory. The state is pinned to its adapter instance ID. Only a
provably dead lock owner can be reclaimed; active profiles are never taken over.
Incomplete or corrupt lock-owner records fail closed and require inspection while
all adapter processes are stopped.
On cleanup timeout during a pending state write, the lock remains held until the
process exits, preserving single ownership.

Storage intentionally fails closed rather than dropping pending data: at most 256
pending inbound messages, 10,000 successful outgoing receipts and 24 MiB state.
There is no silent receipt expiration. `inbound_full`, `receipts_full` or
`state_full` requires operator investigation/capacity migration while the adapter
is stopped. Preserve pending messages and receipt IDs during any migration.

A crash after the provider accepts a message but before receipt persistence can
still cause a duplicate on retry. This SDK cannot promise exactly-once delivery
without provider idempotency. It only guarantees replay of inbound events already
spooled locally; fetching events missed while disconnected is the provider's job.

## Offline example and checks

`../examples/echo.js` has an optional initialization config `peer_id`/`greeting`
for seeding a synthetic conversation and echoes outgoing text as inbound events.
Use it with fake agents to avoid automated reply loops. It requires no npm
dependencies or provider quota.

Run the adapter-root checks in [the development README](../README.md). The tests
cover strict framing, private state, acknowledgement isolation, process crashes,
receipt replay, signal/EOF cleanup and bounded provider calls.
