//! Content-addressed atomic staging and expiration.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{open_regular_with_attributes, read_bounded, AcceptedAttachment};

/// T-32b staging directory under the compose root. Tempfiles live
/// here, named by the content blake3 hash so identical content
/// dedups to a single file across sessions and across agents.
pub fn staging_dir(compose_root: &Path) -> PathBuf {
    compose_root.join("state/attachments-staging")
}

/// Write `accepted.bytes` to the staging dir under a content-
/// addressed name. Idempotent: if the file already exists with the
/// expected content hash, we skip the write (operator may have read the
/// same attachment recently). Returns the staged path so the agent
/// can `read_file()` it directly.
pub fn stage_to_tempfile(
    staging_dir: &Path,
    accepted: &AcceptedAttachment,
) -> Result<PathBuf, std::io::Error> {
    if accepted.size != accepted.bytes.len() as u64
        || accepted.blake3_hex != blake3::hash(&accepted.bytes).to_hex().as_str()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "attachment metadata does not match captured bytes",
        ));
    }
    fs::create_dir_all(staging_dir)?;
    let path = staging_dir.join(&accepted.blake3_hex);
    let cached = open_regular_with_attributes(&path, true)
        .ok()
        .filter(|file| {
            file.metadata().is_ok_and(|m| m.len() == accepted.size)
                && read_bounded(file, accepted.size).ok().is_some_and(|bytes| {
                    blake3::hash(&bytes).to_hex().as_str() == accepted.blake3_hex
                })
        });
    if let Some(file) = cached {
        // Refresh TTL through the validated handle, with no unchecked reopen.
        file.set_modified(SystemTime::now())?;
    } else {
        // Unique, exclusive temporary names avoid races between agents staging
        // identical bytes. Every publication replaces the target atomically.
        let mut tmp = tempfile::NamedTempFile::new_in(staging_dir)?;
        tmp.write_all(&accepted.bytes)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path).map_err(|e| e.error)?;
    }
    Ok(path)
}

/// T-32b: drop tempfiles whose mtime is older than `now - ttl`.
/// Called on team-mcp startup as a best-effort cleanup. Returns the
/// number of files reaped so callers can log a single summary line.
/// Errors traversing individual entries are logged-and-skipped at
/// the call site (kept out of this function so unit tests stay
/// trace-free).
pub fn sweep_expired(staging_dir: &Path, ttl: Duration) -> std::io::Result<usize> {
    if !staging_dir.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(ttl)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut reaped = 0usize;
    for entry in fs::read_dir(staging_dir)? {
        let entry = entry?;
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if mtime < cutoff && fs::remove_file(entry.path()).is_ok() {
            reaped += 1;
        }
    }
    Ok(reaped)
}
