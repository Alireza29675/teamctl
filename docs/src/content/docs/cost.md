---
title: Cost and rate limits
---
Each TeamCTL agent uses its configured runtime and provider account. Agent usage follows that provider's plan, prices and limits. TeamCTL is open source; model usage can still cost money.

This page covers:

- How agents incur cost.
- What TeamCTL shows you today.
- What's planned.
- How to keep the bill predictable.

## How agents incur cost

Every agent is a separate process running Claude Code, Codex, OpenCode or Gemini CLI. TeamCTL does not proxy model calls or pool the agents into one model session. Check provider usage records for actual spend.

This means:

- **Per-agent usage follows the runtime's billing.** Depending on the account, that may mean a subscription allowance or metered API usage.
- **Cost-per-team** is the sum of its agents over the same window.
- **Runtimes can use different provider accounts.** Configure the credentials and spending controls appropriate to each runtime. Several agents may share account-level limits.

## What TeamCTL shows you today

### `teamctl inspect <agent>`

A snapshot of one agent's state:

```bash
teamctl inspect ops:builder
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

Each runtime declares `rate_limit_patterns` in its descriptor (bundled into the binary; sources live at `crates/team-core/runtimes/<name>.yaml`). When TeamCTL sees a rate-limit signal in an agent's output, it triggers the configured `rate_limit_hooks` (wait, send a message, hit a webhook, run a command). Default hook chain: wait until the rate-limit window resets.

Rate-limit detection is per-runtime and **today** matches Claude Code's, Codex's, and Gemini's textual rate-limit messages. Recent Claude Code releases moved to an interactive dialog; the wrapper handles dialog dismissal but the textual pattern may need a refresh as the upstream UX evolves.

## What's planned

- **Cost parsers** for each runtime (Claude Code's `/cost`, Codex's per-message totals, Gemini's summary), feeding the existing `budget` table so the USD column in `teamctl budget` becomes real.
- **Per-agent token-spend rollup in `teamctl ps` and `teamctl budget --by-agent`.** Today you read invoices; planned: a single-pane number per agent across a window.
- **Provider-side budget hooks** — block (or pause) an agent that crosses a per-day soft cap before the provider does it for you.
- **Spend dashboards** — a future docs-site or local web UI surface. Out of scope for v1.

## Keeping the bill predictable

Practical things you can do today to stay in control:

- **Use cheaper models for workers, premium models for managers.** `model: claude-opus-4-8` on a manager + `model: claude-sonnet-4-6` on workers is a common pattern. Set per-agent in `team-compose.yaml`.
- **Set a `daily_usd_limit` in your compose file.** Today this is documentary: the limit shows up in `teamctl budget` next to the (currently always-zero) USD column and anchors operator expectations. Real enforcement lands when cost parsers do.
- **Set `message_ttl_hours`.** `teamctl gc` removes old acknowledged messages. Unread mail is retained, so inspect inbox growth as well as the TTL. Context use still depends on what agents read.
- **Cap the team size.** Three or four agents is plenty for most workflows. If you find yourself wanting eight, ask whether two of them are doing the same job.
- **Review runtime permissions for new roles.** Permission modes differ by runtime; match tool access to the intended scope and inspect early work.
- **Ask agents to request approval for expensive actions.** The role should call `request_approval` before proceeding. The sensitive-action list alone does not intercept tools or enforce a spending cap.

## When agents stop on their own

TeamCTL does not silently keep retrying when an agent hits its limit. The `rl-watch` wrapper detects rate-limit signals, runs the configured hook chain (default: wait until reset), and only then re-spawns. You won't get a runaway respawn loop burning tokens against a closed door.

## Common patterns

### "Three workers, one premium manager, $20/day cap"

```yaml
budget:
  daily_usd_limit: 20.0
  message_ttl_hours: 72

managers:
  lead:
    runtime: claude-code
    model: claude-opus-4-8

workers:
  alpha: { runtime: claude-code, model: claude-sonnet-4-6, reports_to: lead }
  beta:  { runtime: codex,       reports_to: lead }
  gamma: { runtime: gemini,      reports_to: lead }
```

This is one possible mix of roles and runtimes. Provider accounts determine billing and rate-limit windows; different agents or CLIs can still share limits. The declared USD cap is policy metadata until cost tracking and enforcement are implemented.

---

**See also:** [Coordination policy](/coordination-policy/) for how TeamCTL bounds mailbox volume (which directly affects per-turn cost).
