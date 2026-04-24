//! SQLite mailbox schema shared by `team-mcp` and integration tests.
//!
//! The actual connection handling lives in `team-mcp`; this module defines
//! the schema + migrations so both crates agree on the shape of the data.
//!
//! The intentionally minimal Phase 1 schema: `projects`, `agents`,
//! `messages`. Phases 4/5 add `channels`, `bridges`, `approvals`.

/// Idempotent schema bootstrap. Safe to run on every connect.
pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
    id         TEXT PRIMARY KEY,          -- "<project>:<agent>"
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    runtime    TEXT NOT NULL,
    is_manager INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS agents_project_idx ON agents(project_id);

CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT NOT NULL,
    sender       TEXT NOT NULL,            -- "<project>:<agent>" or "user:<handle>" or "cli"
    recipient    TEXT NOT NULL,            -- "<project>:<agent>" or "channel:<project>:<name>"
    text         TEXT NOT NULL,
    thread_id    TEXT,
    sent_at      REAL NOT NULL,
    delivered_at REAL,
    acked_at     REAL
);

CREATE INDEX IF NOT EXISTS messages_recipient_idx
    ON messages(recipient, acked_at);
CREATE INDEX IF NOT EXISTS messages_project_idx
    ON messages(project_id, sent_at);
"#;
