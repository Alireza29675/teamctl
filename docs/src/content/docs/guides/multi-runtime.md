---
title: Multi-runtime teams
---

Mix Claude Code, Codex, OpenCode, and Gemini freely inside one team. They all speak MCP over stdio against the same mailbox; the manager doesn't know or care which CLI a worker happens to be running.

## Declaring runtimes

Each runtime lives in `runtimes/<name>.yaml`:

```yaml
# runtimes/claude-code.yaml
binary: claude
supports_mcp: true
default_model: claude-opus-4-8
```

Reference one from an agent spec:

```yaml
workers:
  dev1:
    runtime: codex
    model: gpt-5-codex
    reports_to: manager
```

The `agent-wrapper.sh` dispatches on `$RUNTIME` and calls the matching binary with the right flags.

## Delivery parity

Agents on any runtime react to new mail on arrival — only the mechanism differs. Claude Code agents get each message pushed into their session as a `<channel source="team">` event; Codex, OpenCode, and Gemini agents get a short `📬 N new team message(s)` note typed into their tmux pane by the team mailbox, which sends them to `inbox_peek` / `inbox_read` / `inbox_ack`. The mailbox is the source of truth in both cases, so mixing runtimes never changes what a message means — just how it knocks.

## What each runtime supports

Not every agent-spec field applies to every runtime:

| Capability | Claude Code | Codex | OpenCode | Gemini |
|---|---|---|---|---|
| MCP (`mcps:` + built-in `team` mailbox) | ✓ | ✓ | ✓ | ✓ |
| `model` | ✓ | ✓ | ✓ | ✓ |
| `effort` | ✓ | ✓ | — | — |
| `permission_mode` (incl. `attended`, `bypassPermissions`) | ✓ | ✓ | ✓ \* | — |
| Compaction on rate limit | ✓ | ✓ | — | — |
| Telegram slash passthrough | ✓ | ✓ | ✓ | — |
| `hooks` | ✓ | — | — | — |
| `subagents` | ✓ | — | — | — |
| `skills` | ✓ | — | — | — |

\* `permission_mode: bypassPermissions` on opencode degrades to `--auto` — opencode has no full-bypass upstream, so deny rules stay enforced; `teamctl validate` prints a warning naming the downgrade.

A capability declared on a runtime that doesn't support it is ignored at render time. `teamctl validate` prints a warning for each such mismatch (validation still succeeds).

One subtlety on MCP env: `${VAR}` placeholders in `mcps:` env values are Claude-Code-only — Claude Code expands them at launch, but Codex and OpenCode pass the literal `${VAR}` string to the server. For codex and opencode agents, give servers literal values or put the secret in the operator environment the MCP server process inherits.

## When to pick which

| Runtime | Strong at |
|---|---|
| Claude Code · Opus | planning, orchestrating, long system prompts |
| Claude Code · Sonnet | fast, cheap tool use; frontend refactors |
| Codex · GPT-5 | deep reasoning on complex backend patches |
| Gemini · 3.0 Pro | 1M-token context for research / large-corpus reads |
| OpenCode · any provider | provider-agnostic mixing; always pin `model:` (`provider/model`) — its own default is the priciest authed model |

## Cost

Each runtime reports cost differently. `teamctl budget` aggregates whatever has been recorded in the `budget` table. Runtime-specific cost parsers are pluggable and land with the runtime adapter itself.

## Example

See `examples/multi-runtime/` — one Claude-Code manager directs a Codex backend dev and a Gemini researcher.
