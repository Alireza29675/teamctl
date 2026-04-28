---
title: "Reference: `runtimes/*.yaml`"
---

Each YAML file under `runtimes/` defines one runtime adapter. File stem = runtime id used in agent `runtime:` fields.

## Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `binary` | string | yes | CLI binary name (resolved on `$PATH`) or absolute path. |
| `supports_mcp` | bool | no, default `false` | Must be `true` for participation in the mailbox. All shipped runtimes set this. |
| `session_resume` | string | no | How sessions resume across restarts: `--continue`, `profile`, `none`, or a runtime-specific flag. |
| `default_model` | string | no | Used when an agent doesn't set its own `model:`. |
| `env` | map<string,string> | no | Merged into the agent's environment on launch. |

## Shipped adapters

- `claude-code.yaml` — Anthropic's Claude Code CLI.
- `codex.yaml` — OpenAI's Codex CLI.
- `gemini.yaml` — Google's Gemini CLI.

## Example

```yaml
# runtimes/aider.yaml — hypothetical adapter for aider
binary: aider
supports_mcp: true
session_resume: "none"
default_model: sonnet
env:
  AIDER_CHAT_HISTORY_FILE: ""
```

You'd also need a `run_aider` branch in `bin/agent-wrapper.sh` that knows how to pass `--mcp-config` and `--system-instruction` to `aider`.
