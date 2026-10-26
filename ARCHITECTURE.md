# Architecture and extension seams

teamctl owns local agent lifecycle; SQLite carries messages and approvals; runtime CLIs own conversations. The public CLI, YAML, and MCP tool surfaces stay at crate boundaries.

| Crate | Responsibility | Main seams |
|---|---|---|
| `team-core` | Compose types, validation, runtime descriptors, rendered artifacts, mailbox schema, durable conversations and persistence | `Supervisor`, rendering modules, `attachments::Scanner`, atomic persistence and process locks |
| `teamctl` | Operator commands and lifecycle reconciliation | `cmd/up` separates artifacts, registration, trust, registry and freshening; snapshot/reload describes applied state; executable adapter host |
| `team-mcp` | Agent-facing tools and authorized mailbox access | Small `tools`/`store` facades delegate to domain modules; authorization remains at the repository boundary |
| `team-bot` | Telegram transport and durable mailbox delivery | Configuration, inbound, outbound, approval, media, repository and transport modules; offline HTTP fixtures |
| `teamctl-ui` | Dashboard state, navigation, rendering and runtime IO and human Chat view | `app` state/actions/events/navigation/render/runtime; injectable pane, mailbox, approval and message adapters |

## Invariants

- Applied snapshots describe successfully started agents. Skipped or failed launches retain their prior applied records. Scoped reloads preserve other agents' unapplied changes; unchanged reloads also recover stopped sessions.
- Lifecycle and registry read/modify/write use process locks. Atomic replacement protects snapshots from interrupted writes. Teardown uses persisted session identities when configuration has changed.
- Runtime `binary`, `args`, environment and model defaults flow through quoted generated launchers. Built-in adapters retain their provider-specific setup; custom adapters receive standard context paths through environment variables.
- Inbox acknowledgement is authorized for the caller. Channel consumption is recorded per subscriber in additive `message_receipts`; global channel acknowledgement occurs once all current non-sender subscribers have consumed it. Previously acknowledged rows remain consumed. Upgrade the CLI, MCP and bot together so older writers do not bypass the new receipt behavior.
- Telegram outgoing work is durable and acknowledged after successful delivery. A provider-success/local-recording crash can produce a duplicate on retry. Media placeholders stay hidden; completed media publishes atomically under a fresh id. A crash during media staging requires an operator retry.
- Attachment policy opens bounded regular-file handles, validates the kernel descriptor path against allowed roots, scans a private snapshot, and stages the same captured bytes atomically. Linux needs `/proc/self/fd`; macOS uses `F_GETPATH`, Windows uses the final handle path. Unknown platforms fail closed for attachment reads. Scanners have bounded diagnostics and owned process-group termination on Unix.
- UI selection follows identities across refreshes. Split rendering, capture and keyboard routing use one cell mapping. Removal of the active target exits streaming mode. Explicit root/environment selection and configured broker paths govern reads and actions.

## Human conversations and messaging transports

`team-core::chat` owns typed conversation/message identities, authorization,
transactional ingress and replies, request receipts and delivery leases. Its
additive schema retains transcripts across inbox GC. Local CLI/UI clients share
one local/operator conversation per canonical agent. Remote transcripts are
readable there, with sends originating from their messenger.

`team-mcp` routes `reply_to_user` to an explicit conversation or inbound reference,
rejects ambiguous/foreign routes and validates multipart capabilities before an
atomic batch. A stable request ID covers the whole batch. Legacy Telegram manager
calls remain available. Unsupported typing/reactions return supported:false.

Telegram's native bot feeds the same repository while retaining approval/media/
voice behavior. Other transports use the [executable protocol](adapters/PROTOCOL.md):
no database access, exact peer/participant allowlists, bounded JSONL IO and owned
process cleanup. The [Node SDK](adapters/sdk/README.md) persists inbound events until
host acknowledgement and outgoing provider receipts. The supplied
[personal WhatsApp adapter](adapters/whatsapp/README.md) adds QR authentication in
private operator state; it is an optional dependency, outside the Rust runtime.

`up`/`reload`/`down` reconcile transports using separately persisted exact session
identities and fingerprints. Root/global locks reserve names before spawn; state
and login directories stay outside all agent workspaces. Foreground pair/identify
commands support configuration without logging secrets or sending external replies.

## Extending safely

Add a runtime descriptor before adding provider-specific wrapper behavior. Test launcher argv/environment with a fake executable; add a built-in adapter only when its MCP, resume and bootstrap contracts are known. Keep secret values out of generated descriptors and tests.

Implement new messengers against protocol v1; advertise only supported capabilities and use stable provider IDs. Add core messaging behavior in its domain tool plus repository method, with authorization and transactional semantics covered by SQLite tests. Test Telegram behavior through the transport seam rather than a real chat. Preserve durable queue rows on transient failure.

Add UI actions through events/actions, keep rendering pure, and exercise them with injected adapters. Use interaction tests for focus and modal transitions, snapshot tests for visible output, and the isolated real tmux contract for resize behavior.

The [verification runbook](CONTRIBUTING.md#verification-runbook) is the canonical local check procedure.
