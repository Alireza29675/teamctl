---
title: Browser workspace
---

Run `teamctl serve` from anywhere inside a `.team/` project. It opens a messenger connected to every configured agent. Start agents separately with `teamctl up`; closing the page or stopping the web server leaves agents running.

```sh
teamctl serve
teamctl serve --no-open --port 8788
teamctl serve --cache-days 7
```

The access link contains a per-launch browser capability in its URL fragment. The client removes that fragment, keeps the capability in this tab's session storage and authenticates every API/asset request. Restarting the server without a persistent token creates a new link. Do not share the link with people who should not control your agents. The server accepts loopback addresses only. For remote access, forward its port through SSH (for example `ssh -L 8787:127.0.0.1:8787 host`) and use the printed localhost link. `--token-env <NAME>` uses a persistent token of at least32 URL-safe characters when you need stable local access.

## Messaging and inspection

Choose an agent or channel in the roster. Agent chat supports Markdown, pasted/dropped files and images, and audio recording where the browser provides microphone access. Each message's provenance disclosure shows provider, instance name, description and ID. Channels currently carry text. Search the visible conversation; load earlier pages for older history. Browser polling updates transcripts every few seconds.

Inspect shows runtime state, hierarchy, pending inbox, tools/MCP names, interface grants, approvals, costs and activity. Cost totals show only reported ledger values; absence of a cost entry does not mean free inference. Approvals support binary and named choices; expired or already-decided requests cannot be approved again.

Agent console shows live tmux output and accepts literal input or explicit Escape/interrupt keys. Native agents have an execution log and receive messages in Chat. Select Workspace shell and Connect to open an on-demand terminal in that agent's directory with its rendered environment. The interactive shell closes when the server stops; deliberately detached jobs follow ordinary shell semantics. Container agents do not silently fall back to a host shell.

## Rich content and storage

Images/audio/files are copied into `.team/state/web/assets`, served through authenticated opaque IDs and retained for30days by default. Agent-origin files pass the configured attachment allow-root/size/scanner policy before caching. Uploads default to25MiB, with ten files per message. Files with executable or unrecognized content types download as attachments. Original filenames are metadata, never server paths.

```yaml
web:
  host: 127.0.0.1
  port: 8787
  cache_retention_days: 30 # null disables age expiry
  max_upload_bytes: 26214400
```

Restart `serve` after changing web settings. Cleanup runs at startup and hourly; expired attachment metadata remains so history can explain unavailable bytes. Abandoned files from interrupted transactions are cleaned after a day. Back up the broker database and the asset directory together if you need durable media beyond the configured cache policy.

Agents send Markdown with `reply_to_user`, images/files/audio with a path source, and self-contained HTML with `html`. Voice recordings reach the agent as audio attachments; understanding audio requires a speech-capable tool or transcription service assigned to that agent. HTML runs in a sandboxed iframe: inline scripts/styles work, parent APIs and external requests do not. Do not expect a component to access model credentials or manipulate your workspace.

## Disposable demo and verification

```sh
fixture=$(mktemp -d)
teamctl --root "$fixture" serve --demo --no-open
```

The demo refuses real teams. It creates two simulated agents and an interactive component, and responds without calling providers. PressCtrl+C to stop. Source tests use disposable roots, mocked providers and private tmux sockets. See CONTRIBUTING for the current verification runbook. Live-provider tests are separate and may consume model quota.
