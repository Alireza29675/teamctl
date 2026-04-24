//! SQLite mailbox schema shared by `team-mcp` and integration tests.
//!
//! The actual connection handling lives in `team-mcp`; this module defines
//! the schema + migrations so both crates agree on the shape of the data.
//!
//! The intentionally minimal Phase 1 schema: `projects`, `agents`,
//! `messages`. Phases 4/5 add `channels`, `bridges`, `approvals`.

/// Idempotent schema bootstrap. Safe to run on every connect.
pub const SCHEMA: &str = r#"
-- NOTE: pragmas (journal_mode=WAL, busy_timeout, foreign_keys) are set by
-- the connection opener *before* this batch runs — concurrent openers race
-- if we set them here.

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

-- Phase 2: channels + subscriptions + per-agent ACLs.
CREATE TABLE IF NOT EXISTS channels (
    id         TEXT PRIMARY KEY,               -- "<project>:<name>"
    project_id TEXT NOT NULL,
    name       TEXT NOT NULL,
    wildcard   INTEGER NOT NULL DEFAULT 0       -- 1 iff members = "*"
);

CREATE TABLE IF NOT EXISTS channel_members (
    channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    agent_id   TEXT NOT NULL,
    PRIMARY KEY (channel_id, agent_id)
);

CREATE INDEX IF NOT EXISTS channel_members_agent_idx
    ON channel_members(agent_id);

CREATE TABLE IF NOT EXISTS agent_acls (
    agent_id        TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    can_dm_json     TEXT NOT NULL DEFAULT '[]',    -- ["dev","critic"]
    can_bcast_json  TEXT NOT NULL DEFAULT '[]'     -- ["product","all"]
);

-- Phase 4: inter-project manager bridges.
CREATE TABLE IF NOT EXISTS bridges (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    from_agent   TEXT NOT NULL,             -- "<project>:<agent>", must be a manager
    to_agent     TEXT NOT NULL,             -- "<project>:<agent>", must be a manager
    topic        TEXT NOT NULL,
    opened_by    TEXT NOT NULL,             -- "user:<handle>" or "cli"
    opened_at    REAL NOT NULL,
    expires_at   REAL NOT NULL,
    closed_at    REAL
);

CREATE INDEX IF NOT EXISTS bridges_open_idx
    ON bridges(expires_at, closed_at);
"#;
