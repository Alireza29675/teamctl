//! Atomic files and portable process locks for persisted state.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Write a private sibling temporary file, flush it, and atomically replace
/// the destination. A failed write leaves the last complete version intact.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Serialize a read-modify-write operation across processes. SQLite's
/// transaction lock is released on connection drop, including process
/// crashes, so no stale pid file can permanently block a later command.
pub struct ProcessLock {
    _connection: Connection,
}

impl ProcessLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open state lock {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.execute_batch("BEGIN IMMEDIATE").with_context(|| {
            format!(
                "acquire state lock {} (another command may be running)",
                path.display()
            )
        })?;
        Ok(Self { _connection: conn })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_keeps_complete_contents_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn dropped_process_lock_can_be_reacquired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.lock.sqlite");
        drop(ProcessLock::acquire(&path).unwrap());
        drop(ProcessLock::acquire(&path).unwrap());
    }
}
