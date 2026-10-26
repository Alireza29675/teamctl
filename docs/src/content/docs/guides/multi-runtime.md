---
title: Multi-runtime teams
---

Choose Claude Code, Codex, OpenCode or Gemini CLI for each agent. Mix runtimes or use several sessions of the same one; all connect to the same team mailbox. Roles and relationships remain your choice.

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

Agents on any runtime react to new mail on arrival — only the mechanism differs. Claude Code agents get each message pushed into their session as a `<channel source="team">` event; Codex, OpenCode, and Gemini agents get a short note typed into their tmux pane by the team mailbox — `📬 sender: "short preview…" (+N more)` — which sends them to `inbox_peek` / `inbox_read` / `inbox_ack`. Because the note is typed as keystrokes, the preview is sanitized: collapsed to a single line with all control characters stripped, so a message body can never submit input or drive the agent's TUI. The mailbox is the source of truth in both cases, so mixing runtimes never changes what a message means — just how it knocks.

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

## Choose for the role

Check the tools, permission controls, model availability and account limits the role needs. Test a representative interaction before assigning ongoing work. For OpenCode, pin `model:` in `provider/model` form instead of relying on its default selection.

## Cost

Each runtime reports usage differently. `teamctl budget` sums its recorded ledger, but runtime cost parsers are not wired today, so its USD column does not measure actual spend. Use provider usage records and spending controls; see [Cost and rate limits](/cost/).

## Example

See [Product team](https://github.com/Alireza29675/teamctl/tree/main/examples/product-team) for Claude Code and Codex agents, or [Newsletter office](https://github.com/Alireza29675/teamctl/tree/main/examples/newsletter-office) for Claude Code, Codex and Gemini CLI across two projects.
