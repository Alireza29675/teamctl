# Runtimes

A **runtime** is the CLI binary behind an agent. teamctl ships adapters for the three major AI coding CLIs — they can all mix freely inside one team.

| Runtime | Binary | MCP | Session resume | Notes |
|---|---|---|---|---|
| Claude Code | `claude` | yes | `--continue` | The default. Strongest for planning + tool use. |
| Codex CLI | `codex` | yes (0.14+) | profile | OpenAI's CLI. Good for deep reasoning on patches. |
| Gemini CLI | `gemini` | yes (0.3+) | n/a (loop-restart) | 1M-token context makes it great for research. |

Adapters live under `runtimes/<name>.yaml`:

```yaml
# runtimes/claude-code.yaml
binary: claude
supports_mcp: true
session_resume: "--continue"
default_model: claude-opus-4-7
env:
  CLAUDE_PROJECT_DIR_MODE: compose
```

Referenced from an agent spec:

```yaml
workers:
  dev1:
    runtime: codex
    model: gpt-5-codex
```

## Adding a new runtime

1. Drop a `runtimes/<yourcli>.yaml` with at least `binary:`.
2. Extend `bin/agent-wrapper.sh` with a `run_yourcli` branch that shells out with the right flags.
3. Run `teamctl reload`.

If the binary is missing on `$PATH`, `teamctl up` fails fast with a clear error rather than spawning a doomed tmux session.

## Related

- [Guide: Multi-runtime teams](../guides/multi-runtime.md)
- [Reference: runtimes/*.yaml](../reference/runtimes-yaml.md)
