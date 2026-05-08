//! SQLite mailbox schema shared by `team-mcp` and integration tests.
//!
//! The actual connection handling lives in `team-mcp`; this module defines
//! the schema + migrations so both crates agree on the shape of the data.

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
    is_manager INTEGER NOT NULL DEFAULT 0,
    reports_to TEXT                        -- short name, resolved within project
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

-- Channels + subscriptions + per-agent ACLs.
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

-- Inter-project manager bridges.
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

-- Human-in-the-loop permission fabric.
CREATE TABLE IF NOT EXISTS approvals (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id     TEXT NOT NULL,
    agent_id       TEXT NOT NULL,
    action         TEXT NOT NULL,          -- "publish", "deploy", ...
    scope_tag      TEXT,                   -- optional narrower tag
    summary        TEXT NOT NULL,
    payload_json   TEXT,
    status         TEXT NOT NULL,          -- pending | approved | denied | expired | undeliverable
    requested_at   REAL NOT NULL,
    decided_at     REAL,
    decided_by     TEXT,
    decision_note  TEXT,
    expires_at     REAL NOT NULL,
    delivered_at   REAL                    -- NULL until an interface adapter confirms surfacing to a human
);

CREATE INDEX IF NOT EXISTS approvals_pending_idx
    ON approvals(status, expires_at);

-- Budget ledger. Rows are appended by interface adapters and by runtime
-- cost parsers. `teamctl budget` aggregates per project/day.
CREATE TABLE IF NOT EXISTS budget (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT NOT NULL,
    agent_id    TEXT,
    runtime     TEXT,
    usd         REAL NOT NULL DEFAULT 0,
    input_tok   INTEGER NOT NULL DEFAULT 0,
    output_tok  INTEGER NOT NULL DEFAULT 0,
    observed_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS budget_project_day_idx
    ON budget(project_id, observed_at);

-- Rate-limit events. Written by `teamctl rl-watch` whenever a runtime
-- emits a rate-limit signature. Hooks (notify, webhook, run) run off these
-- rows; the wrapper loop sleeps until `resets_at` before respawning.
CREATE TABLE IF NOT EXISTS rate_limits (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL,
    runtime     TEXT NOT NULL,
    hit_at      REAL NOT NULL,
    resets_at   REAL,                  -- nullable: sometimes we can't parse
    raw_match   TEXT NOT NULL,
    handled_at  REAL
);

CREATE INDEX IF NOT EXISTS rate_limits_agent_idx
    ON rate_limits(agent_id, hit_at);
"#;

/// Bootstrap the schema and apply additive migrations. Idempotent — safe on
/// every connect. Replaces direct `execute_batch(SCHEMA)` calls so that
/// existing databases pick up new columns without a destructive reset.
pub fn ensure(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Additive migrations. SQLite has no `ADD COLUMN IF NOT EXISTS`, so each
    // migration tolerates the "duplicate column name" error to stay idempotent.
    let migrations: &[&str] = &[
        "ALTER TABLE approvals ADD COLUMN delivered_at REAL",
        // T-086-A: discriminator + structured payload for non-text mailbox kinds
        // (image, file, reaction). Existing text rows have NULL on both — readers
        // treat NULL kind as 'text' for back-compat.
        "ALTER TABLE messages ADD COLUMN kind TEXT",
        "ALTER TABLE messages ADD COLUMN structured_payload TEXT",
        // T-086-B: Telegram message id this row pertains to. Direction-
        // disambiguated by sender: inbound rows (sender = `user:telegram`)
        // store the source Telegram message id so agents know what to
        // reply to; outbound rows (sender = `<project>:<agent>`) store the
        // id this reply threads under for `reply_parameters`. NULL on
        // pre-T-086-B rows and on rows that aren't Telegram-bound.
        "ALTER TABLE messages ADD COLUMN telegram_msg_id INTEGER",
    ];
    for stmt in migrations {
        if let Err(e) = conn.execute(stmt, []) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(e);
            }
        }
    }
    Ok(())
}
