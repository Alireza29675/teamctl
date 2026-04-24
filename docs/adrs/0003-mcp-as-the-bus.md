# ADR 0003 — MCP as the inter-agent bus

- Status: accepted
- Date: 2026-04-24

## Context

We need an IPC protocol that every supported runtime (Claude Code, Codex CLI, Gemini CLI) already speaks. Options: a bespoke JSON-over-socket, HTTP, gRPC, or MCP.

## Decision

Use **MCP stdio** (protocol version `2024-11-05`) as the only inter-agent bus. Every runtime launches with `--mcp-config` pointing at a per-agent config that loads `team-mcp` as a stdio server.

## Rationale

- Zero integration work per runtime — all three adapters already ship MCP support.
- Tools are discoverable: agents see `dm`, `broadcast`, `inbox_watch` in the tool list and can call them without prompt engineering.
- Keeps the "runtime agnostic" promise real. A new CLI that speaks MCP joins the mailbox for free.
- Avoids inventing a protocol the runtimes would have to be patched to support.

## Consequences

- We version against a single MCP protocol version per release and feature-flag new fields.
- MCP over stdio per-agent means one `team-mcp` process per agent (spawned by the wrapper). They all share one SQLite file. Acceptable — processes are cheap, SQLite WAL handles concurrent writers.
- Tool metadata (`x-action-tag`) is the primary hook for HITL gating (ADR 0001 forthcoming).
