# Decisions

Lighter architectural decisions on this repo — the kind that deserve a written reason but don't warrant a full ADR. Each entry: date, title, status, one-paragraph why. Load-bearing decisions (new runtime adapter, compose-schema break, HITL semantic shift) graduate to the formal series under `docs/src/content/docs/adrs/`.

## 2026-04-24 — SQLite WAL over Redis Streams for the mailbox

**Status:** accepted. **Why:** keeps the host-native promise (no Docker, no broker daemon) intact while delivering sub-5 ms inbox latency; WAL matches the N-agents-1-broker topology and OS-level notify wakes agents without polling. Full context in [ADR 0001](docs/src/content/docs/adrs/0001-sqlite-over-redis.md).

## 2026-04-24 — MCP stdio as the only inter-agent bus

**Status:** accepted. **Why:** Claude Code, Codex CLI, and Gemini CLI all ship MCP support out of the box, so the bus needs zero per-runtime integration work and tools (`dm`, `broadcast`, `inbox_watch`) appear in the agent's tool list discoverably. Full context in [ADR 0003](docs/src/content/docs/adrs/0003-mcp-as-the-bus.md).

## 2026-05-02 — One Telegram bot per user-facing manager

**Status:** accepted (implemented in v0.6.0). **Why:** the prior single-global-bot design forced operators to type `/dm <project>:<role> <text>` for every message and walk a six-step setup doc. Per-manager bots let operators DM each manager directly and use Telegram's drafts and reply threads naturally; `teamctl bot setup` is the wizard that wires BotFather → token → `/start` → chat id once per manager. Full context in [ADR 0005](docs/src/content/docs/adrs/0005-per-manager-telegram-bot.md).
