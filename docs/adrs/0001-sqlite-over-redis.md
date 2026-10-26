---
title: ADR 0001 — SQLite mailbox over Redis Streams
---

- Status: accepted
- Date: 2026-04-24

## Context

teamctl needs a message broker for the agent mailbox. Candidates: SQLite WAL, Redis Streams, NATS JetStream, PostgreSQL LISTEN/NOTIFY.

## Decision

Start with **SQLite in WAL mode**. Migrate only if we hit a concrete need for pub/sub broadcast with history replay beyond what triggers + polling cover.

## Rationale

- Zero new daemons. The whole value prop is "host-native, no Docker, no Redis." Adding a broker service works against that.
- Sub-5 ms latency on inserts and on the `inbox_watch` path (proven by Overstory).
- WAL allows concurrent readers + one writer, which matches the N-agents-1-broker topology exactly.
- `notify` (inotify on Linux, FSEvents on macOS) wakes agents on new messages without polling.

## Consequences

- Broadcast fanout is a query, not a subscription; for small teams this is fine, but tens of agents on one channel will stress it.
- There is no replay semantics; `messages` rows carry a TTL and are dropped after `message_ttl_hours`.
- The `broker.type: redis-streams` escape hatch exists in the schema for when we outgrow SQLite.
