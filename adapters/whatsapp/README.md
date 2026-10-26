# WhatsApp adapter

The supplied process host requires macOS or Linux. Ownership inspection uses
SQLite's Unix reserved lock in rollback-journal mode; native Windows hosting is
unsupported. Linux remains unverified by this local macOS run.

This opt-in transport connects a personal WhatsApp account by QR pairing, using
[whatsapp-web.js 1.34.7](https://github.com/wwebjs/whatsapp-web.js). It supports text
and quoted text replies. The upstream project is an unofficial WhatsApp Web
client and states that account blocking is possible; review its
[upstream disclaimer](https://github.com/wwebjs/whatsapp-web.js#disclaimer) before
pairing an account. It is distributed under Apache-2.0; the teamctl wrapper and
SDK use MIT.

## Configure and pair

Add an interface to the existing `interfaces` list in `team-compose.yaml`:

```yaml
interfaces:
  - name: personal-whatsapp
    type: whatsapp
    config:
      agent: demo:manager
      allow_senders: ["123456789@c.us"]
      allow_peers: ["123456789@c.us"]
      options:
        # Optional: use an already-installed browser instead of the bundled one.
        browser_executable_path: /absolute/path/to/chrome
        reconnect_limit: 3
```

Replace the agent with a known canonical agent and use exact provider identifiers.
WhatsApp may report a sender as an opaque `@lid` identifier; do not infer or rewrite
it as a phone number. For groups, allow the exact `@g.us` peer and the permitted
participant's `@c.us` or `@lid` sender. Both allowlists must match; empty lists deny
all ingress. The host owns authorization and routing.

Run from the team root:

```sh
teamctl adapter setup personal-whatsapp
teamctl adapter pair personal-whatsapp
teamctl adapter identify personal-whatsapp
teamctl adapter run personal-whatsapp
```

Setup requires Node 22.12+ and npm. A normal `npm ci` downloads Puppeteer's browser
if needed. To use an existing configured browser, set `PUPPETEER_SKIP_DOWNLOAD=true`
during setup. Pairing must be an interactive foreground session: QR payloads are
ephemeral credentials and must never be piped to logs or saved. Background run
reports pairing_required while unpaired and never emits the QR secret.

The host stores login/profile/cache state outside repositories and agent working
directories. A single SDK lock owns each instance. The browser keeps its sandbox,
receives an AbortSignal and cannot take over another active WhatsApp session.
Ordinary shutdown closes the browser without logging out. If graceful close
stalls, the adapter terminates/reaps its owned browser process group.

## Optional phone self-chat

To control an agent from your own account's "Message yourself" chat, explicitly
enable this transport option:

```yaml
options:
  self_chat: true
```

After pairing, run `teamctl adapter identify personal-whatsapp` and send a text
note to yourself **from the WhatsApp phone app**. This foreground command prints
the note's exact `peer_id` and `sender_id` without forwarding it to an agent or
sending a reply. Stop it with Ctrl+C, copy those IDs into `allow_peers` and
`allow_senders`, then run the adapter normally. These IDs may use `@c.us` or `@lid`;
keep their exact values. Ordinary allowed incoming contacts and group participants
continue to work with this option enabled. Self-chat remains disabled by default.

The adapter handles own notes through upstream's `message_create` event. Both the
sender and destination must match the connected account's `Client.info.wid`, or
phone/LID aliases returned by `getContactLidAndPhone` for that account. Alias lookup
is bounded; a failed lookup accepts only the known account ID. IDs are preserved
in the normalized event, and the host still checks both allowlists. Own messages
to other contacts or groups are ignored.

Only upstream `deviceType` values `android` and `ios` qualify; web, desktop and
unknown origins are ignored. Upstream [classifies device type using message ID
format](https://github.com/wwebjs/whatsapp-web.js/blob/v1.34.7/src/structures/Message.js),
so this is a compatibility heuristic rather than a guaranteed origin check.
The adapter also suppresses pending-send text echoes and remembers up to 10,000
recent outgoing source IDs for the current process. A same-text phone note while
its matching reply is being sent may be suppressed conservatively. These guards
reduce reply loops but cannot make upstream device classification infallible.
If phone notes are not recognized, leave self-chat disabled and use an allowed
incoming contact or group participant. Paired phone self-chat and its behavior
across real device/network changes have not been verified live.

## Behavior and limits

The adapter preserves group and participant identities, source message IDs and
quoted references. It ignores own messages unless phone self-chat is enabled,
status updates, broadcasts, channels
and inbound non-text events. Outgoing media/control requests fail explicitly with
unsupported_kind. Quoted sends validate the referenced message's peer and reject
an unavailable/foreign quote instead of silently sending unquoted text. Sends
disable preview fetching and automatic read/seen side effects.

Ready, authentication failure, disconnect, conflict and logout are separate
events. Transient disconnects get bounded exponential retries (default 3, maximum 10,
up to 30 seconds between attempts). Old clients lose their callbacks. Authentication
failure/logout needs explicit recovery; conflicts never take over another session.
The SDK spools inbound events until host acknowledgement and stores successful
outbound receipts; see its [delivery guarantees and capacity limits](../sdk/README.md).
The provider cannot guarantee replay of messages missed while disconnected.

The root lockfile retains WhatsApp 1.34.7 and overrides its pinned Puppeteer 24.38.0
with Puppeteer 25.13.0 / browsers 3.2.4. This removes the extract-zip/FTP dependency
advisories from the upstream browser-download tree. The adapter closes through
Puppeteer 25's public `Browser.close()` before `Client.destroy()` because upstream
WhatsApp's destructor checks the removed `isConnected()` method. The manually
invoked upstream `Client.logout()` still uses that removed method and is not part
of this adapter's supported lifecycle. No browser takeover or sandbox disabling
flags are introduced.

As verified 2026-10-10, `npm ci --ignore-scripts`, 46 offline tests, syntax checks and
`npm audit` pass with zero reported vulnerabilities. Installed-source/public-API
inspection, a real about:blank browser smoke and an unpaired QR-generation smoke
also passed. No account was linked and no external message was sent. Sending with
a paired account, real network reconnection and other operating systems remain
unverified. The dependencies fluent-ffmpeg/glob carry upstream deprecation
warnings even though npm reports no current vulnerabilities.

## Optional browser checks

These commands are excluded from `npm test`. Use an existing browser executable;
every smoke uses a disposable private profile and removes its profile/cache.
Run from the adapter installation/development root:

```sh
node whatsapp/scripts/browser-smoke.js --executable /absolute/path/to/chrome
```

This opens only about:blank, checks the public APIs used by the text/QR path and
closes/reaps the browser. To separately test unpaired WhatsApp Web compatibility:

```sh
node whatsapp/scripts/browser-smoke.js --executable /absolute/path/to/chrome --unpaired-qr
```

The latter navigates WhatsApp Web for at most 60 seconds until a QR appears, ignores
the QR payload, and reports only outcome metadata. Never scan/link the account or
send a message during this check. It also destroys/reaps the browser and deletes
its profile/cache. These scripts are developer tools and are not bundled into the
built-in adapter runtime assets.
