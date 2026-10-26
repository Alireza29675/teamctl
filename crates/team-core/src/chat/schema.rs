pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS chat_adapter_capabilities (
    adapter_id TEXT PRIMARY KEY,
    capabilities_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_conversations (
    id TEXT PRIMARY KEY,
    adapter_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    UNIQUE(adapter_id, peer_id, agent_id)
);
CREATE INDEX IF NOT EXISTS chat_conversations_agent_idx
    ON chat_conversations(agent_id);
CREATE INDEX IF NOT EXISTS chat_conversations_updated_idx
    ON chat_conversations(updated_at DESC, id);

CREATE TABLE IF NOT EXISTS chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES chat_conversations(id),
    direction TEXT NOT NULL CHECK(direction IN ('human', 'agent')),
    text TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT,
    mailbox_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    external_id TEXT,
    reply_to_external_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('queued', 'sent', 'failed', 'received')),
    error TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS chat_messages_history_idx
    ON chat_messages(conversation_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS chat_messages_mailbox_idx
    ON chat_messages(mailbox_id) WHERE mailbox_id IS NOT NULL;

-- Provider replay IDs belong to a peer, even when configuration changes its agent.
CREATE TABLE IF NOT EXISTS chat_sources (
    adapter_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    external_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    message_id INTEGER NOT NULL REFERENCES chat_messages(id),
    PRIMARY KEY(adapter_id, peer_id, external_id)
);
CREATE TABLE IF NOT EXISTS chat_requests (
    caller TEXT NOT NULL,
    request_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    message_id INTEGER NOT NULL REFERENCES chat_messages(id),
    PRIMARY KEY(caller, request_id)
);
CREATE TABLE IF NOT EXISTS chat_deliveries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL UNIQUE REFERENCES chat_messages(id),
    adapter_id TEXT NOT NULL,
    lease_owner TEXT,
    lease_until REAL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at REAL NOT NULL,
    finished INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS chat_deliveries_ready_idx
    ON chat_deliveries(adapter_id, finished, next_attempt_at, lease_until);
"#;
