---
title: Multi-runtime teams
---

Mix Claude Code, Codex, and Gemini freely inside one team. They all speak MCP over stdio against the same mailbox; the manager doesn't know or care which CLI a worker happens to be running.

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

Agents on any runtime react to new mail on arrival — only the mechanism differs. Claude Code agents get each message pushed into their session as a `<channel source="team">` event; Codex and Gemini agents get a short note typed into their tmux pane by the team mailbox — `📬 sender: "short preview…" (+N more)` — which sends them to `inbox_peek` / `inbox_read` / `inbox_ack`. Because the note is typed as keystrokes, the preview is sanitized: collapsed to a single line with all control characters stripped, so a message body can never submit input or drive the agent's TUI. The mailbox is the source of truth in both cases, so mixing runtimes never changes what a message means — just how it knocks.

## When to pick which

| Runtime | Strong at |
|---|---|
| Claude Code · Opus | planning, orchestrating, long system prompts |
| Claude Code · Sonnet | fast, cheap tool use; frontend refactors |
| Codex · GPT-5 | deep reasoning on complex backend patches |
| Gemini · 3.0 Pro | 1M-token context for research / large-corpus reads |

## Cost

Each runtime reports cost differently. `teamctl budget` aggregates whatever has been recorded in the `budget` table. Runtime-specific cost parsers are pluggable and land with the runtime adapter itself.

## Example

See `examples/multi-runtime/` — one Claude-Code manager directs a Codex backend dev and a Gemini researcher.
