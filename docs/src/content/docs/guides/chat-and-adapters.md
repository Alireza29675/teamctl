---
title: Chat and messaging adapters
---

Run `teamctl up` to initialize the broker and register agents, then open a local
conversation with any known canonical agent:

```sh
teamctl chat demo:manager
teamctl chat send demo:manager "Hello" --request-id hello-1
teamctl chat list --json
teamctl chat history demo:manager --limit 100 --json
teamctl chat follow demo:manager
```

Interactive CLI: Enter sends; Ctrl+C/Ctrl+D exits. Use the UI multiline composer
or `chat send` with a quoted multiline argument when composing several lines.
`history` accepts a conversation ID or agent, with `--before` / `--after` paging.
`follow` streams additions and revisions in its latest 200-message window. Reuse
`--request-id` only for retries of the same submission; changed content rejects.

In the dashboard, press `c` for Chat. Tab cycles conversations, history and
composer. Enter adds a newline; Esc then Enter or Alt/Ctrl+Enter sends. Ctrl+D
returns to the dashboard. Each conversation keeps its own session-local draft;
failed sends preserve the draft. PageUp/PageDown browse history and End follows
new replies. CLI and UI resume the same local conversation. Remote or removed-agent
threads remain readable; remote replies originate in the messenger.

An agent replies with `reply_to_user`, passing `conversation_id` or
`reply_to_message_id` from its inbound mailbox `meta.id`. Channel events include
conversation, adapter, peer, source and chat-message metadata. Any agent can reply
to its own human conversation. A manager's old unaddressed Telegram calls retain
the legacy behavior. Use an explicit route when more than one conversation exists.
A cached request retry confirms the original enqueue and status; it does not send
a fresh message. Queued media needs its original source until transport delivery.

The supplied executable adapter host requires macOS or Linux. Native Windows
hosting is unsupported because ownership inspection uses Unix SQLite locks.
This local verification ran on macOS.

## Personal WhatsApp

Add this top-level interface to `team-compose.yaml`, replacing the agent with a
canonical ID from your roster:

```yaml
interfaces:
  - name: personal-whatsapp
    type: whatsapp
    config:
      agent: demo:manager
      allow_senders: []
      allow_peers: []
      options:
        self_chat: true
        reconnect_limit: 3
```

Setup needs Node 22.12+ and npm; it installs pinned optional adapter dependencies
in operator state. Pairing needs an interactive terminal:

```sh
teamctl adapter setup personal-whatsapp
teamctl adapter pair personal-whatsapp
teamctl adapter identify personal-whatsapp
```

Scan the QR from WhatsApp's linked-device screen. QR data is an ephemeral
credential: do not pipe pairing output to a log. After pairing, `identify` prints
exact peer and sender IDs without forwarding text or sending replies. Send a note
from the phone app, stop identify with Ctrl+C, and copy the exact IDs into both
allowlists. Start the host with `teamctl adapter start personal-whatsapp` after
the broker has been initialized. Empty allowlists deny messages; groups require
both the group's peer and an allowed participant sender. Preserve opaque `@lid`
IDs without guessing a phone number.

`self_chat` is opt-in for phone-origin notes to your own account. It uses upstream's
message-ID device heuristic and verified account aliases to reduce reply loops.
Web/unknown-origin own messages and own sends to other chats are ignored. Paired
self-chat/device changes remain unverified live; an allowed incoming contact/group
is the fallback if phone notes are not recognized.

The provided adapter uses unofficial
[whatsapp-web.js](https://github.com/wwebjs/whatsapp-web.js), whose upstream warns
that account blocking is possible. It supports text/quotes; unsupported media or
controls fail explicitly. Profile, browser and receipt/spool state stay outside
repositories and agent directories. Ordinary stop preserves login. See the
[adapter README](https://github.com/Alireza29675/teamctl/blob/main/adapters/whatsapp/README.md)
for browser options, delivery limits, dependency pins and verification scope.

## Lifecycle and diagnostics

```sh
teamctl adapter list
teamctl adapter run personal-whatsapp       # foreground
teamctl adapter stop personal-whatsapp      # just this transport
teamctl reload --dry-run
```

`up` starts declared process transports; `reload` reconciles changed, removed or
stopped transports, with project/agent scopes following their target. `down` stops
the persisted process identities even after configuration removal or prefix change.
Only one host owns an instance. Stop the old managed identity before moving a
foreground instance to another target, or reconcile both targets together. After
changing paired options/allowlists, reload
or restart it. Background hosts report pairing_required without exposing QR data.

Transcript status distinguishes queued, sent and failed delivery. Sent means the
transport/provider accepted the send; it does not promise the human read it.
History is retained until operator-managed retention/export is added; inbox GC
has a separate policy. Treat transcripts and private account state as sensitive.
Inspect controlled host errors in its tmux pane; provider stderr is discarded.

## Write an adapter

Use `type: process` with an executable path and literal argv:

```yaml
interfaces:
  - name: custom-chat
    type: process
    config:
      agent: demo:manager
      command: /absolute/path/to/adapter
      args: ["--foreground"]
      env:
        PROVIDER_TOKEN: CUSTOM_PROVIDER_TOKEN
      allow_senders: ["person:3"]
      allow_peers: ["chat:7"]
      startup_timeout_secs: 90
      options: {}
```

The `env` values name operator environment variables, rather than containing
credentials. `teamctl env --doctor` checks these references. Adapter executables
are trusted operator code. The host supplies
routing and private state through the versioned stdin/stdout protocol; adapters
own provider authentication, capabilities and connection recovery. `local` and
`telegram:` instance names are reserved. Never write directly to SQLite.

The canonical [protocol](https://github.com/Alireza29675/teamctl/blob/main/adapters/PROTOCOL.md),
[Node SDK](https://github.com/Alireza29675/teamctl/blob/main/adapters/sdk/README.md)
and [offline echo example](https://github.com/Alireza29675/teamctl/blob/main/adapters/examples/echo.js)
are included in the source tree. No account or model quota is needed for the
normal adapter test suite.
