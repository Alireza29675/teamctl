# team-mcp internals

The binary exposes the existing JSON-RPC/MCP API over stdio. `rpc.rs` owns
framing, `tools.rs` owns caller context and dispatch, and `tools/schema.rs`
owns the advertised tool schemas. Tool implementations live under `tools/`,
grouped by messaging, inboxes, human replies, attachments, approvals, and tmux.

`store.rs` owns the SQLite connection and shared row types. Domain methods
live under `store/`; they use the same connection and transaction policy.
The schema and migrations remain in `team_core::mailbox` so every process
agrees on persisted state.

Mailbox rules to preserve when adding tools or consumers:

- Direct messages are consumed only by their recipient. Inbox reads and
  explicit acknowledgements enforce the same visibility rules.
- Each channel subscriber consumes independently through `message_receipts`.
  Historical global acknowledgements stay consumed; new broadcasts receive a
  global acknowledgement after all current subscribers except the sender
  have consumed them.
- `media_pending` is staging state and is hidden from inbox consumers. The
  bot publishes completed media with a fresh message id.
- Watcher queries apply their high-water filter before the batch limit, so
  an unconsumed backlog cannot hide newer notifications.
- Malformed persisted ACL data and database errors return errors rather
  than granting permission.

From the repository root, run `cargo test -p team-mcp` for unit tests and
stdio/channel/approval integration tests. Domain unit tests are stored under
`store/tests.rs` and `tools/tests/`.
