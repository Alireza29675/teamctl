# team-bot internals

`main.rs` contains shared runtime state and the binary entry point.
`config.rs` preserves the public CLI flags; `startup.rs` initializes the
mailbox and Telegram dispatcher. The remaining modules separate inbound
routing, the shared-chat bridge, approval callbacks, outbound polling, SQLite queries, transport,
media, voice transcription, bounded downloads, tmux commands, and rendering.

Delivery rules to preserve when adding transport features:

- An empty authorized-chat list permits `/start` discovery only. Invalid
  chat ids fail startup; configured chats retain their existing access.
- Ordinary text, explicit `/dm`, successful voice transcripts, media and
  `media_error` publications bridge into `team_core::chat`. The stable
  adapter identity is `telegram:<project>:<manager>`; the legacy unscoped
  instance uses `telegram:legacy`. Chat IDs and human sender IDs remain exact
  opaque strings, and source Telegram message IDs deduplicate within their
  chat. Telegram's existing `user:telegram` and threading fields stay intact.
- New shared-chat replies use their recorded, currently authorized peer.
  Provider acceptance and returned Telegram message IDs are recorded under
  durable delivery leases. Unsupported outgoing content fails visibly;
  transport failure leaves the delivery eligible for bounded retry.
- Startup and native ingress declare `text`, `image`, `file`, `typing` and
  `reaction` capabilities for the adapter instance. Shared typing requests
  require `expires_at` in Unix seconds; expired or malformed requests finish
  with an error instead of reopening a typing window after restart. Refresh
  lasts until that deadline with the existing ten-second ceiling. Shared
  reactions keep the native `{telegram_msg_id, emoji}` payload and use the
  conversation's authorized peer.
- The authorized operator's explicit `/dm` retains access to any registered
  agent; its reply returns to the originating chat through that explicit
  conversation. Ordinary text still targets the configured manager. Agents
  cannot reply into another agent's conversation.
- Legacy outbound polling selects persisted unacknowledged messages and pending
  undelivered approvals, including work queued before restart. Successful
  transport delivery consumes a row; failure retains it for retry with
  bounded backoff. These older rows have no recorded peer and retain the
  first-configured-chat fallback. Approval and unsolicited legacy delivery
  remain scoped to the configured manager's reporting chain.
- Delivery is at least once: a crash after Telegram accepts a send but
  before SQLite records success can cause a duplicate on restart.
- Approval callbacks check expiry in the atomic decision update and enforce
  manager routing. A delivered card is not repeatedly sent while awaiting
  its decision. Delivery is recorded only while the approval remains live;
  cards accepted after expiry or resolution are removed.
- Inbound routing confirmations follow successful persistence. Media uses
  a hidden staging row for the download path, then publishes a new row
  atomically with its conversation metadata while preserving the inbound
  Telegram message id. A failed conversation write rolls publication back
  to hidden staging. A crash
  before publication leaves staging state and requires the operator to retry.
  Final file flush errors publish `media_error` rather than completed media.
- Reply-context lookup uses adapter and peer identity. Unknown legacy peers
  remain unresolved instead of borrowing another chat's matching message ID.
  Authorized anonymous-admin messages use their stable, nonprivileged
  `chat:<sender_chat.id>` identity, including updates with Telegram's shared
  bot proxy or no `from` field. The destination chat must still be authorized.
  Ordinary bot echoes without `sender_chat` are refused;
  authorized `/start`, slash controls and approval callbacks retain their
  existing behavior. Run only one Telegram poller for a configured bot.
- Text and captions retain their complete valid body at Telegram's UTF-16
  limits. Attribution is omitted when it would exceed the limit. Oversized
  CLI text is split at Unicode scalar boundaries before each chunk is rendered.
- The renderer's output and public CLI defaults are covered by the existing
  feature tests.

From the repository root, run `cargo test -p team-bot`. Legacy feature tests
live under `src/tests/`; `src/regression_tests.rs` exercises persistence and
the production sender against a local Telegram API substitute. These tests
do not require Telegram credentials or a paid model session.
