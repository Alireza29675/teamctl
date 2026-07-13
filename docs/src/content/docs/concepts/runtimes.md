---
title: Runtimes
---

A **runtime** is the CLI binary behind an agent. teamctl ships adapters for the three major AI coding CLIs — they can all mix freely inside one team.

| Runtime | Binary | MCP | Session resume | Notes |
|---|---|---|---|---|
| Claude Code | `claude` | yes | always-on (deterministic UUID; `--session-id` to create, `--resume` to attach) | The default. Strongest for planning + tool use. |
| Codex CLI | `codex` | yes (via per-agent `CODEX_HOME` config.toml) | profile | OpenAI's CLI. Good for deep reasoning on patches. |
| Gemini CLI | `gemini` | yes (0.3+) | n/a (loop-restart) | 1M-token context makes it great for research. |

For Claude Code, every spawn uses a UUIDv5 derived deterministically from `teamctl:<project>:<agent>`. The wrapper passes `--session-id <uuid>` to create the session on first spawn, and `--resume <uuid>` on every subsequent spawn once the session jsonl exists. Same UUID either way, so an agent's context survives `teamctl down`/`up`, crash recovery, and host reboots without operator action. If the session-file at that UUID is ever removed (manual cleanup, claude session-dir reset), the wrapper falls back to `--session-id` and claude recreates a fresh session at the same UUID. Self-healing by construction.

Adapters live under `runtimes/<name>.yaml`:

```yaml
# runtimes/claude-code.yaml
binary: claude
supports_mcp: true
default_model: claude-opus-4-8
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

- [Guide: Multi-runtime teams](/guides/multi-runtime/)
- [Reference: runtimes/*.yaml](/reference/runtimes-yaml/)
