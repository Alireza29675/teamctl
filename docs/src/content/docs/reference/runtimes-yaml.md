---
title: "Reference: `runtimes/*.yaml`"
---

Each YAML file under `runtimes/` defines one runtime adapter. File stem = runtime id used in agent `runtime:` fields.

## Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `binary` | string | yes | CLI binary name (resolved on `$PATH`) or absolute path. |
| `supports_mcp` | bool | no, default `false` | Must be `true` for participation in the mailbox. All shipped runtimes set this. |
| `session_resume` | string | no | Parsed-but-inert metadata, kept for back-compat. The actual resume strategy is hard-coded per runtime in `agent-wrapper.sh`: claude-code spawns with a deterministic UUIDv5 derived from `teamctl:<project>:<agent>` (`--session-id` to create, `--resume` to attach existing); codex resumes via `--profile`; gemini has no equivalent. Setting this in a custom adapter is a no-op today. |
| `default_model` | string | no | Used when an agent doesn't set its own `model:`. |
| `env` | map<string,string> | no | Merged into the agent's environment on launch. |

## Shipped adapters

- `claude-code.yaml`: Anthropic's Claude Code CLI.
- `codex.yaml`: OpenAI's Codex CLI.
- `gemini.yaml`: Google's Gemini CLI.

## Example

```yaml
# runtimes/aider.yaml: hypothetical adapter for aider
binary: aider
supports_mcp: true
default_model: sonnet
env:
  AIDER_CHAT_HISTORY_FILE: ""
```

> `session_resume` was an early field for declaring resume strategy.
> It's still parsed (back-compat) but inert: leave it out of new
> adapters. claude-code agents now auto-resume via deterministic
> UUIDv5 session ids; other runtimes either resume natively or don't.

You'd also need a `run_aider` branch in `bin/agent-wrapper.sh` that knows how to pass `--mcp-config` and `--system-instruction` to `aider`.
