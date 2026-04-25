# Rate limits

Every runtime hits limits eventually — Claude Max sessions reset every 5h,
Codex usage caps reset on a rolling window, Gemini quota fires on the hour.
teamctl detects these hits, runs whatever you tell it to do about them,
and **waits until the limit clears** before respawning the runtime —
instead of hammering the API in a tight retry loop.

## How it works

Every runtime invocation runs through `teamctl rl-watch`:

```
agent-wrapper.sh
  → teamctl rl-watch <project>:<agent> -- claude --mcp-config … --model …
       │
       ├── stdout/stderr piped through, echoed to the tmux pane
       ├── each line tested against the runtime's `rate_limit_patterns`
       ├── on first match:
       │     • write a row to `rate_limits` (agent_id, runtime, hit_at,
       │       resets_at, raw_match)
       │     • run the agent's hook chain in order
       │     • exit 0
       └── otherwise: pass through the runtime's exit code
```

After `rl-watch` exits, the wrapper's `while :; do … sleep 5; done` loop
respawns the runtime. The `wait` hook is what makes that respawn happen
*after* the limit window — without it the wrapper would just retry into
the same wall.

## Patterns

Each `runtimes/<name>.yaml` declares one or more detection patterns:

```yaml
rate_limit_patterns:
  - match: "(?i)limit reached.*resets at"
    resets_at_capture: "(?i)resets at ([0-9]{1,2}(?::[0-9]{2})?\\s*(?:am|pm)?)"

  - match: "(?i)rate.?limit"
    resets_in_capture: "(?i)(?:retry|wait)\\s*(?:in|after)?\\s*([0-9]+\\s*(?:s|m|h)[a-z]*)"
```

- `match` — Rust regex tested against each output line.
- `resets_at_capture` — extracts an absolute clock-time ("4pm", "16:00",
  "16:00 UTC"). Resolved to the *next future* occurrence.
- `resets_in_capture` — extracts a relative duration ("5h 15m", "120s").
  Added to the hit timestamp.

Either capture is optional. If neither resolves, the wait falls back to
`rate_limits.fallback_wait_seconds` (default 30 minutes).

## Hooks

`team-compose.yaml` declares named hooks plus the default chain that
runs on every hit. Agents can override.

```yaml
rate_limits:
  default_on_hit: [notify-tg, wait]
  fallback_wait_seconds: 1800

  hooks:
    - name: notify-tg
      action: send
      to: "startup:product_manager"
      template: "⚠️ {agent} rate-limited; resets {resets_at_local}"

    - name: pager
      action: webhook
      url_env: PAGER_WEBHOOK
      method: POST

    - name: top-up
      action: run
      command: ["bin/top-up.sh", "{agent}", "{runtime}"]

# per-agent override:
workers:
  dev1:
    on_rate_limit: [pager, top-up, wait]
```

## Hook actions

| Action | What it does |
|---|---|
| `wait` | Sleep until `resets_at` (+ 5 s jitter) or `fallback_wait_seconds`. |
| `send` | Insert a message into the mailbox. The interface adapter (Telegram, etc.) forwards it. Needs `to:` and `template:`. |
| `webhook` | `curl -X <method> --data <event-json> <url>`. `url:` literal or `url_env:` env-var. |
| `run` | `Command::new(command[0]).args(&command[1..])` with placeholders substituted. |

## Placeholders

In templates and `run` arguments:

| Token | Value |
|---|---|
| `{agent}` | `<project>:<agent>` |
| `{runtime}` | `claude-code` / `codex` / `gemini` / … |
| `{hit_at}` | UNIX seconds when the hit was recorded |
| `{resets_at}` | UNIX seconds when the limit clears (or `unknown`) |
| `{resets_at_local}` | Local-time formatted reset time |
| `{raw_match}` | The exact line that matched |

## Inspecting hits

```bash
sqlite3 state/mailbox.db \
  "SELECT id, agent_id, runtime, hit_at, resets_at, raw_match FROM rate_limits ORDER BY id DESC LIMIT 10"
```

A `teamctl rate-limits` view is on the near-term roadmap.
