# Runtimes

A **runtime** is the CLI binary behind an agent. teamctl ships adapters for:

| Runtime | Binary | MCP | Session resume |
|---|---|---|---|
| Claude Code | `claude` | yes | `--continue` |
| Codex CLI | `codex` | yes | profile-based |
| Gemini CLI | `gemini` | yes (0.3+) | `--yolo` (gated by HITL) |

Runtimes are defined declaratively in `runtimes/*.yaml` and referenced per agent:

```yaml
workers:
  dev1:
    runtime: codex
    model: gpt-5-codex
```

`team-mcp` is runtime-agnostic: any CLI that speaks MCP stdio can join the mailbox.

## Related

- [Guide: Multi-runtime teams](../guides/multi-runtime.md)
- [Reference: runtimes/*.yaml](../reference/runtimes-yaml.md)
