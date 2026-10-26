# team-bot internals

`main.rs` contains shared runtime state and the binary entry point.
`config.rs` preserves the public CLI flags; `startup.rs` initializes the
mailbox and Telegram dispatcher. The remaining modules separate inbound
routing, approval callbacks, outbound polling, SQLite queries, transport,
media, voice transcription, bounded downloads, tmux commands, and rendering.

Delivery rules to preserve when adding transport features:

- An empty authorized-chat list permits `/start` discovery only. Invalid
  chat ids fail startup; configured chats retain their existing access.
- Outbound polling selects persisted unacknowledged messages and pending
  undelivered approvals, including work queued before restart. Successful
  transport delivery consumes a row; failure retains it for retry with
  bounded backoff. Conversation replies retain their recorded interface and peer;
  only legacy unaddressed rows use the first configured chat.
- Delivery is at least once: a crash after Telegram accepts a send but
  before SQLite records success can cause a duplicate on restart.
- Approval callbacks check expiry in the atomic decision update and enforce
  configured agent routing. A delivered card is not repeatedly sent while awaiting
  its decision. Delivery is recorded only while the approval remains live;
  cards accepted after expiry or resolution are removed.
- Inbound routing confirmations follow successful persistence. Media uses
  a hidden staging row for the download path, then publishes a new row
  atomically while preserving the inbound Telegram message id. A crash
  before publication leaves staging state and requires the operator to retry.
  Final file flush errors publish `media_error` rather than completed media.
- Text and captions retain their complete valid body at Telegram's UTF-16
  limits. Attribution is omitted when it would exceed the limit. Oversized
  CLI text is split at Unicode scalar boundaries before each chunk is rendered.
- The renderer's output and public CLI defaults are covered by the existing
  feature tests.

From the repository root, run `cargo test -p team-bot`. Legacy feature tests
live under `src/tests/`; `src/regression_tests.rs` exercises persistence and
the production sender against a local Telegram API substitute. These tests
do not require Telegram credentials or a paid model session.

Managed launches pass `--token-env NAME` and `--stt-api-key-env NAME`, so the
long-lived tmux command and process arguments contain reference names only.
The bot resolves those references from `<compose-root>/.env`, then the parent
`.env`, then its inherited environment. A local empty value stays empty;
old tmux server variables cannot override rotated team credentials. Dotenv
values are read literally, without shell evaluation. Keep dotenv files private
and out of version control. Manual literal token flags remain available;
environment-backed help output hides their values.

For a manual launch from a compose root, prefer:

```bash
team-bot --mailbox state/mailbox.db --compose-root . --token-env MY_BOT_TOKEN --agent personal:companion
```
