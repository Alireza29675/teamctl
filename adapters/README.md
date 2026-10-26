# Messaging adapters

The supplied process host requires macOS or Linux. Ownership inspection uses
SQLite's Unix reserved lock in rollback-journal mode; native Windows hosting is
unsupported. Linux remains unverified by this local macOS run.

An adapter is an executable that exchanges protocol-v1 JSON lines with teamctl.
The host owns routing, exact sender/peer allowlists, conversations, delivery leases
and retries. Adapters own provider connections and private transport state. They
never open the team mailbox database.

- [Protocol v1](PROTOCOL.md): language-independent executable contract.
- [Node SDK](sdk/README.md): dependency-free reusable package and transport API.
- [WhatsApp](whatsapp/README.md): supplied opt-in personal QR adapter.
- [Echo example](examples/echo.js): offline executable using the same SDK. It
  emits synthetic replies; use an isolated fake agent when exercising it.

The terminal CLI and dashboard Chat view are local clients of the shared chat
repository. Telegram keeps its native transport and existing media/voice/approval
features. Community adapters can use any language that implements the executable
contract; Node is a convenience rather than a requirement.

## Development checks

Run from `adapters/` with Node 22.12 or newer:

```sh
PUPPETEER_SKIP_DOWNLOAD=true npm ci --ignore-scripts
npm test
npm run check
npm audit
```

The default tests use mock WhatsApp clients and disposable SDK subprocesses.
They neither launch a browser nor contact WhatsApp. Lockfile installs retain
WhatsApp 1.34.7 with an explicit Puppeteer 25.13.0 override, removing the upstream
Puppeteer 24 browser-download dependency advisories. Recheck compatibility and the
audit when changing either dependency; see the WhatsApp README for verification
limits and an explicit opt-in browser smoke.

## Packaged assets

The built-in adapter installation copies this root's package manifests and
lockfile, `sdk/package.json` + `sdk/src/`, and `whatsapp/package.json` +
`whatsapp/src/` into operator state, then runs `npm ci` there. Runtime entrypoint:
`node <installed-assets>/whatsapp/src/main.js`. Profiles, caches and spool/receipt
files belong to the host-provided `state_dir`, outside repositories and agent
working directories. Neither `node_modules` nor account state is embedded in the
Rust executable.
