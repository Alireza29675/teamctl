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
session_resume: "--continue"
default_model: claude-opus-4-7
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
