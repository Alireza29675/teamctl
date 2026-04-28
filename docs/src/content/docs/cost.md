---
title: Cost & rate limits
---




If you're running multiple agents at once, *cost per agent* and *cost per team* are the questions that matter most. teamctl is built around the fact that **each agent is a real CLI session calling a real provider API** — so the cost model maps cleanly onto what your provider already invoices.

This page covers:

- How agents incur cost.
- What teamctl shows you today.
- What's planned.
- How to keep the bill predictable.

## How agents incur cost

Every agent in a teamctl team is a separate process running `claude`, `codex`, or `gemini`. Each one makes its own API calls under its own credentials. There is no shared LLM and no central proxy — token spend is whatever the provider records for that session.

This means:

- **Cost-per-agent = cost-per-session of the underlying CLI.** If you can read your Anthropic / OpenAI / Google invoice today, you already know what an agent cost.
- **Cost-per-team** is the sum of its agents over the same window.
- **Different runtimes can use different keys.** A Claude Code worker, a Codex worker, and a Gemini worker in the same team can each be billed to a different provider account — useful for rate-limit headroom and provider-level budget controls.

## What teamctl shows you today

### `teamctl inspect <agent>`

A snapshot of one agent's state:

```bash
teamctl inspect solo:manager
```

The output includes the agent's project / role / runtime / model / role-prompt / supervisor state / tmux session / paths to its rendered env and MCP files, the last 10 messages it sent or received, and any recent rate-limit hits recorded by `rl-watch`. It does **not** report token spend — there is no `/cost` integration today, on any runtime. To see per-agent spend, read your provider's invoice or its CLI's own cost command (`/cost` inside a Claude Code session, equivalent for Codex / Gemini).

### `teamctl budget`

A team-wide activity surface:

```bash
teamctl budget
teamctl budget --project newsroom
```

For each project today, it prints message count (24 h), approval count (24 h), USD-24 h, and agent count, alongside the per-project or global limit declared in compose:

```yaml
budget:
  daily_usd_limit: 20.0
  per_project_usd_limit:
    newsroom: 5.0
  message_ttl_hours: 72
```

**Honesty note on the USD column.** The schema and the aggregator are wired (a `budget` table with a `usd` column; the SQL sums it per project per day). What is **not** wired is any cost parser feeding rows into that table — Claude Code's `/cost`, Codex's per-message totals, and Gemini's summary are all on the roadmap, not in the binary today. So today the column reads `0.00$` regardless of real spend. The mailbox-side of the budget (TTL) is enforced by `teamctl gc` and is real today.

### Rate-limit detection

Each runtime declares `rate_limit_patterns` in its descriptor (bundled into the binary; sources live at `crates/team-core/runtimes/<name>.yaml`). When teamctl sees a rate-limit signal in an agent's output, it triggers the configured `rate_limit_hooks` (wait, send a message, hit a webhook, run a command). Default hook chain: wait until the rate-limit window resets.

Rate-limit detection is per-runtime and **today** matches Claude Code's, Codex's, and Gemini's textual rate-limit messages. Recent Claude Code releases moved to an interactive dialog; the wrapper handles dialog dismissal but the textual pattern may need a refresh as the upstream UX evolves.

## What's planned

- **Cost parsers** for each runtime (Claude Code's `/cost`, Codex's per-message totals, Gemini's summary), feeding the existing `budget` table so the USD column in `teamctl budget` becomes real.
- **Per-agent token-spend rollup in `teamctl ps` and `teamctl budget --by-agent`.** Today you read invoices; planned: a single-pane number per agent across a window.
- **Provider-side budget hooks** — block (or pause) an agent that crosses a per-day soft cap before the provider does it for you.
- **Spend dashboards** — a future docs-site or local web UI surface. Out of scope for v1.

## Keeping the bill predictable

Practical things you can do today to stay in control:

- **Use cheaper models for workers, premium models for managers.** `model: claude-opus-4-7` on a manager + `model: claude-sonnet-4-6` on workers is a common pattern. Set per-agent in `team-compose.yaml`.
- **Set a `daily_usd_limit` in your compose file.** Today this is documentary: the limit shows up in `teamctl budget` next to the (currently always-zero) USD column and anchors operator expectations. Real enforcement lands when cost parsers do.
- **Set `message_ttl_hours`.** Old messages get garbage-collected by `teamctl gc`. Smaller mailbox → smaller per-call context → fewer tokens per turn.
- **Cap the team size.** Three or four agents is plenty for most workflows. If you find yourself wanting eight, ask whether two of them are doing the same job.
- **Use `permission_mode: confirm` for new role prompts.** Forces the manager to approve each tool call until you trust the role. Reduces runaway loops.
- **Tag your most expensive tool calls in `hitl.globally_sensitive_actions`.** They'll block on `request_approval` — you can deny at the moment of cost.

## When agents stop on their own

teamctl does not silently keep retrying when an agent hits its limit. The `rl-watch` wrapper detects rate-limit signals, runs the configured hook chain (default: wait until reset), and only then re-spawns. You won't get a runaway respawn loop burning tokens against a closed door.

## Common patterns

### "Three workers, one premium manager, $20/day cap"

```yaml
budget:
  daily_usd_limit: 20.0
  message_ttl_hours: 72

managers:
  lead:
    runtime: claude-code
    model: claude-opus-4-7

workers:
  alpha: { runtime: claude-code, model: claude-sonnet-4-6, reports_to: lead }
  beta:  { runtime: codex,       reports_to: lead }
  gamma: { runtime: gemini,      reports_to: lead }
```

Three runtimes mean three independent rate-limit windows. The Sonnet workers carry the bulk of the work; Opus only when the manager needs to think.

---

**See also:** [Coordination policy](/coordination-policy/) for how teamctl bounds mailbox volume (which directly affects per-turn cost).
