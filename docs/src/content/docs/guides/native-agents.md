---
title: Native agents and container sandboxes
description: Run mailbox-driven agents without a terminal and optionally isolate their tools with Docker.
---

Native execution starts a small background teamctl worker for each agent. The worker wakes for messages, starts the selected provider CLI in its documented headless mode, and releases the provider process when its turn ends. Conversations resume using the exact saved provider session ID. Idle agents consume no provider CLI memory.

Agents in one team are trusted collaborators. They share a workspace and writable mailbox; processes with that access can impersonate other agents. Interface grants route normal tool calls and support revocation, while a separate root and narrower mounted volumes provide a stronger boundary between agents that should not trust each other.

```yaml
agents:
  personal:
    runtime: codex # claude-code and opencode also supported
    role_prompt: roles/personal.md
    execution:
      mode: native
      turn_timeout_seconds: 300
      wake_interval_seconds: 900
      sandbox:
        type: host
```

The agent wakes once when first started. Incoming messages wake it immediately. Optional `wake_interval_seconds` schedules further work; the next due time survives restarts. An inbox turn moves the next periodic wake forward, avoiding duplicate scheduled work immediately after a conversation. The minimum wake interval is 10 seconds and each turn has a configurable 1–3600 second timeout. Omitting the wake interval makes the agent event driven.

Use the role prompt to describe what the agent should review during scheduled work and where it should keep its task list and notes. A wake is a bounded work session, so the agent should save progress before returning. Provider tools and MCP servers remain available in each turn.

```sh
teamctl up
teamctl serve
teamctl status
teamctl down
```

`up`, `reload`, `down`, and `status` use the configured execution mode. `--fresh` clears the native session checkpoint before restarting the chosen agent. Execution defaults to `tmux` when omitted; use `mode: tmux` for an attached interactive provider terminal. Native workers offer logs and messenger input; they do not expose a provider TUI.

## Messages and recovery

The worker passes each message's interface provider, ID, name, description, content kind and structured payload to the agent. Its final text returns to the exact source conversation. Explicit `reply_to_user` calls support rich responses and suppress the automatic plain-text final. Agent-to-agent replies use the `dm` or `broadcast` MCP tools, avoiding endless automatic acknowledgment exchanges.

Successful turns acknowledge their inbox message. Failed or timed out turns retain it and retry after 30 seconds; the worker log records the error. Channel acknowledgments are per subscriber, and agents ignore their own broadcasts. Provider session checkpoints and wake times live in `state/runtime-sessions/`; worker logs live in `state/processes/`; the current provider output lives in `state/runtime-output/`.

Mailbox acknowledgments and automatically generated conversation replies commit in one transaction. An interrupted provider turn can run again after restart, so workflows that modify external systems should use idempotency keys where their APIs support them.

## Authentication

Host mode uses the installed CLI's ordinary login. Run `codex login`, `claude auth login`, or `opencode auth login` before starting an agent. Codex gets a per-agent `CODEX_HOME` with a reference to the operator's existing credential file; provider sessions and generated MCP config remain separate for each agent. Claude's noninteractive mode does not require teamctl to alter the global workspace trust config.

Native agents let the provider choose a model when `model` is omitted. An explicit per-agent model or a custom runtime descriptor's `default_model` takes precedence. Check model availability for the account you authenticate; a model listed in a local cache might not be available to that account. Worker health in `state/processes/<project>-<agent>.status.json` records idle/busy/error/stopped state, the latest error and the next scheduled wake.

Container mode receives only the environment variable names and mounts you explicitly declare. Teamctl does not mount your home directory or Docker daemon socket. Provider OAuth stores can contain refreshable tokens: use a dedicated credential mount for the intended agent and allow writes if your provider must refresh it. API credentials can be supplied through named environment references. Keep credentials outside Git and never place secret values directly in YAML.

## Docker sandbox

Build a Linux image containing the provider CLIs and team MCP server:

```sh
docker build -f containers/Dockerfile -t teamctl-agent:local .
```

The Docker daemon must already be running. Then configure the agent:

```yaml
execution:
  mode: native
  turn_timeout_seconds: 300
  sandbox:
    type: docker
    image: teamctl-agent:local
    env: [OPENAI_API_KEY]
    mounts:
      - source: ../notes
        target: /notes
        read_only: false
```

When a provider's own filesystem sandbox cannot run inside an unprivileged container, explicitly set `permission_mode: bypassPermissions` on that agent to use the container as its tool boundary. This policy permits tool execution throughout the declared container mounts; it should be an intentional choice. Host execution keeps the provider's ordinary unattended policy.

The declared image is required; teamctl never silently falls back to host execution. Each active turn runs an ephemeral container on a bridge network shared by the team. The network permits internet access. The workspace and compose root are mounted at their original absolute paths so generated MCP configs, files and the shared mailbox resolve consistently. Extra mount sources resolve against the compose root and targets must be absolute. The generated MCP config uses the Linux `/usr/local/bin/team-mcp` from the image.

Containers run with the invoking user's UID/GID, dropped capabilities, no new privileges, an init process and a persistent per-agent home under `state/container-home/`. Their filesystem and process namespaces isolate tools from the host outside mounted paths. Agents can write the shared workspace and mailbox, so this is a team collaboration trust boundary; use separate roots and narrower mounts for agents that should not trust each other.

The default mount plan masks top-level `.env` and `.env.*` files in both the workspace and compose root. It also hides sibling agents' generated env files and Codex, OpenCode and container credential homes. Named environment grants still supply the selected credentials, and explicit additional mounts remain operator choices. Nested application secrets and the shared broker are inside the granted workspace boundary; these masks do not turn a shared workspace into a malicious-agent security boundary.

The default attachment allowlist uses `$HOME`, which points to the private container home. To send files from a mounted workspace through MCP, explicitly include that workspace's absolute path in the global attachment policy:

```yaml
attachments:
  allowed_roots: [/absolute/path/to/workspace, /notes]
```

Read-only credential mounts are unsuitable for providers that refresh tokens in place. A Codex credential file should target that agent's rendered `state/codex-home/<project>-<agent>/auth.json`; Claude and OpenCode use paths under the container home. API environment reference behavior remains provider owned. Start with `teamctl up`, inspect the worker log for authentication diagnostics, and stop with `teamctl down`.

Last verified: 2026-10-18. Native provider flags were checked against Claude Code 2.1.299, Codex 0.154.0 and OpenCode 1.18.3. A real OpenCode worker completed bootstrap and message turns with the same session. A fake provider in real Docker completed both turns and verified shared networking, internet egress, read/write and read-only mounts, and credential masks. The full provider image still needs an operator image build and provider authentication.
