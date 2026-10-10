//! Append-only audit records with transport-independent metadata.

use std::fs;
use std::io::Write;
use std::path::Path;

/// T-32b audit log entry. Written as a single JSON line per attempt
/// so the file is `tail -f`-friendly + parseable with `jq`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry<'a> {
    /// RFC3339 UTC timestamp.
    pub ts: String,
    /// Operator-supplied path verbatim — preserved so a typo is
    /// debuggable directly from the log.
    pub path: &'a str,
    /// Canonicalized path (`None` when the path failed to resolve).
    pub resolved: Option<&'a str>,
    pub outcome: &'static str,
    pub size: Option<u64>,
    pub blake3: Option<&'a str>,
    pub reason: Option<String>,
}

/// Append a single JSON-line audit entry. No-op when
/// `audit_log_path` is `None`. Errors creating the parent dir or
/// opening the file are surfaced to the caller — production paths
/// log-and-continue so a misconfigured audit dir doesn't block real
/// reads.
pub fn append_audit(audit_log_path: Option<&Path>, entry: &AuditEntry<'_>) -> std::io::Result<()> {
    let Some(p) = audit_log_path else {
        return Ok(());
    };
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut f = fs::OpenOptions::new().append(true).create(true).open(p)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// Helper: format an RFC3339 UTC timestamp suitable for audit
/// entries. Pulled out so tests can pin the format independently of
/// the call site.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
