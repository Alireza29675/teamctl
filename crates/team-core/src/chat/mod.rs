//! Durable human conversations shared by local clients and transport adapters.
//!
//! The transcript outlives disposable agent inbox rows. Callers authorize
//! provider identities before ingestion; replies are always scoped to the agent.

use std::path::Path;

use rusqlite::Connection;

mod delivery;
mod inbound;
mod model;
pub mod protocol;
mod repository;
mod schema;
mod web;

pub use model::{ChatMessage, Conversation, Delivery, DeliveryStatus, Direction, InboundMessage};
pub use repository::Repository;

pub const MAX_PAGE_SIZE: usize = 200;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ID_BYTES: usize = 1024;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} was not found")]
    NotFound(String),
    #[error("unknown agent: {0}")]
    UnknownAgent(String),
    #[error("invalid chat input: {0}")]
    InvalidInput(String),
    #[error("request or source ID was reused with different content")]
    IdempotencyConflict,
    #[error("conversation or message does not belong to this caller")]
    Forbidden,
    #[error("multiple conversations are available; specify a conversation or inbound message ID")]
    AmbiguousRoute,
    #[error("no conversation route is available")]
    NoRoute,
    #[error("delivery lease is no longer owned by this worker")]
    LeaseLost,
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Open the shared mailbox with the same locking and migration policy as MCP.
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut last_error = None;
    for attempt in 0..20 {
        let result = (|| {
            let conn = Connection::open(path)?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            crate::mailbox::ensure(&conn)?;
            Ok(conn)
        })();
        match result {
            Ok(conn) => return Ok(conn),
            Err(Error::Database(error))
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(25 * (attempt + 1)));
            }
            Err(error) => return Err(error),
        }
    }
    Err(Error::Database(
        last_error.unwrap_or(rusqlite::Error::InvalidQuery),
    ))
}

pub(crate) fn ensure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(schema::SCHEMA)?;
    if let Err(error) = conn.execute(
        "ALTER TABLE chat_messages ADD COLUMN interface_json TEXT",
        [],
    ) {
        if !error.to_string().contains("duplicate column name") {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
