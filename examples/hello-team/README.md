# Example: hello-team

> Populated in Phase 1.

The smallest useful teamctl deployment: one project, one manager agent running Claude Code, SQLite mailbox, Telegram optional.

```bash
teamctl up
teamctl status
teamctl send hello:manager "summarise the README"
teamctl logs hello:manager -f
```

See [Getting started](../../docs/getting-started.md).
