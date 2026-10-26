//! Private attachment cache shared by MCP and the browser workspace.
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{io::Write, path::Path};
pub const SCHEMA:&str="CREATE TABLE IF NOT EXISTS web_assets(id TEXT PRIMARY KEY,name TEXT NOT NULL,mime_type TEXT NOT NULL,size INTEGER NOT NULL,created_at REAL NOT NULL,expired_at REAL); CREATE INDEX IF NOT EXISTS web_assets_expiry ON web_assets(created_at,expired_at);";
pub fn ensure(conn: &Connection, root: &Path) -> Result<()> {
    let dir = root.join("state/web/assets");
    std::fs::create_dir_all(&dir)?;
    for path in [root.join("state"), root.join("state/web"), dir.clone()] {
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("web cache directories must not be symlinks");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    conn.execute_batch(SCHEMA)?;
    Ok(())
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn cache_bytes(
    conn: &Connection,
    root: &Path,
    bytes: &[u8],
    name: &str,
    mime: &str,
    max: u64,
) -> Result<Value> {
    if bytes.len() as u64 > max {
        bail!("asset exceeds configured upload limit");
    }
    ensure(conn, root)?;
    let dir = root.join("state/web/assets");
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    let seed = format!("{}:{}:{}", tmp.path().display(), std::process::id(), now());
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed.as_bytes());
    hasher.update(bytes);
    let id = hasher.finalize().to_hex().to_string();
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(dir.join(&id))?;
    let name = safe_name(name);
    let mime = safe_mime(mime);
    let created = now();
    if let Err(e) = conn.execute(
        "INSERT INTO web_assets(id,name,mime_type,size,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![id, name, mime, bytes.len() as i64, created],
    ) {
        let _ = std::fs::remove_file(dir.join(&id));
        return Err(e.into());
    }
    Ok(
        json!({"id":id,"url":format!("/api/assets/{id}"),"mime_type":mime,"name":name,"size":bytes.len(),"created_at":created}),
    )
}
pub fn cache_path(conn: &Connection, root: &Path, path: &Path) -> Result<Value> {
    let compose = crate::compose::Compose::load_read_only(root)?;
    let scanner = compose
        .global
        .attachments
        .scanner
        .as_ref()
        .map(|s| crate::attachments::RealScanner::for_spec(s));
    let bytes =
        crate::attachments::check_and_read(&compose.global.attachments, path, scanner.as_deref())
            .map_err(|e| anyhow::anyhow!(e.human()))?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("attachment");
    cache_bytes(
        conn,
        root,
        &bytes,
        name,
        mime_from_name(name),
        compose.global.web.max_upload_bytes,
    )
}
pub fn descriptor(conn: &Connection, id: &str) -> Result<Option<Value>> {
    if !valid_id(id) {
        return Ok(None);
    }
    conn.query_row("SELECT name,mime_type,size,created_at,expired_at FROM web_assets WHERE id=?1",[id],|r|Ok(json!({"id":id,"url":format!("/api/assets/{id}"),"name":r.get::<_,String>(0)?,"mime_type":r.get::<_,String>(1)?,"size":r.get::<_,i64>(2)?,"created_at":r.get::<_,f64>(3)?,"expired":r.get::<_,Option<f64>>(4)?.is_some()}))).optional().context("read asset metadata")
}
pub fn cleanup(conn: &Connection, root: &Path, days: Option<u32>, timestamp: f64) -> Result<usize> {
    ensure(conn, root)?;
    // Crashes or outer transaction rollbacks can leave bytes without metadata.
    // Delay orphan deletion for a day so concurrent writers can commit safely.
    for entry in std::fs::read_dir(root.join("state/web/assets"))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !valid_id(&name) && !name.starts_with(".tmp") {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(timestamp);
        if timestamp - modified < 86400.0 {
            continue;
        }
        let exists = conn
            .query_row(
                "SELECT 1 FROM web_assets WHERE id=?1",
                [name.as_ref()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            std::fs::remove_file(entry.path())?;
        }
    }
    let Some(days) = days else {
        return Ok(0);
    };
    ensure(conn, root)?;
    let mut stmt =
        conn.prepare("SELECT id FROM web_assets WHERE expired_at IS NULL AND created_at<=?1")?;
    let ids = stmt
        .query_map([timestamp - f64::from(days) * 86400.0], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut count = 0;
    for id in ids {
        if !valid_id(&id) {
            continue;
        }
        match std::fs::remove_file(root.join("state/web/assets").join(&id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        conn.execute(
            "UPDATE web_assets SET expired_at=?2 WHERE id=?1",
            params![id, timestamp],
        )?;
        count += 1;
    }
    Ok(count)
}
pub fn safe_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | '"' | ';'))
        .take(160)
        .collect();
    if name.trim().is_empty() {
        "attachment".into()
    } else {
        name
    }
}
pub fn safe_mime(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "audio/mpeg" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        "audio/wav" => "audio/wav",
        "audio/webm" => "audio/webm",
        "audio/mp4" => "audio/mp4",
        "video/webm" => "video/webm",
        "video/mp4" => "video/mp4",
        "application/pdf" => "application/pdf",
        "text/plain" => "text/plain",
        _ => "application/octet-stream",
    }
}
pub fn mime_from_name(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "webm" => "audio/webm",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expires_bytes_keeps_metadata_and_respects_disable() {
        let root = tempfile::tempdir().unwrap();
        let c = Connection::open_in_memory().unwrap();
        let item = cache_bytes(&c, root.path(), b"hello", "hello.txt", "text/plain", 10).unwrap();
        let id = item["id"].as_str().unwrap();
        let created = item["created_at"].as_f64().unwrap();
        assert_eq!(cleanup(&c, root.path(), None, created + 1e8).unwrap(), 0);
        assert_eq!(
            cleanup(&c, root.path(), Some(30), created + 29.0 * 86400.0).unwrap(),
            0
        );
        assert_eq!(
            cleanup(&c, root.path(), Some(30), created + 30.0 * 86400.0).unwrap(),
            1
        );
        assert!(!root.path().join("state/web/assets").join(id).exists());
        assert_eq!(descriptor(&c, id).unwrap().unwrap()["expired"], true);
    }
    #[test]
    fn rejects_large_and_unsafe_metadata() {
        let root = tempfile::tempdir().unwrap();
        let c = Connection::open_in_memory().unwrap();
        assert!(cache_bytes(&c, root.path(), b"oversize", "x", "text/html", 1).is_err());
        let item = cache_bytes(&c, root.path(), b"x", "../a\r\n.html", "text/html", 10).unwrap();
        assert_eq!(item["mime_type"], "application/octet-stream");
        assert!(!item["name"].as_str().unwrap().contains('/'));
        assert!(descriptor(&c, "../secret").unwrap().is_none());
    }
}
