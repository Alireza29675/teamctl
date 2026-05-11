//! `teamctl whatsnew [version] [--since <from>]` — ad-hoc lookup of
//! GitHub release notes, sharing the post-update renderer.
//!
//! Modes:
//! - No flags: print the running binary's release body.
//! - Positional `version`: print that version's body.
//! - `--since <from>`: aggregate every release in `(from, current]` at
//!   or above the floor, oldest-first, framed and footer'd.
//!
//! Positional `version` is ignored when `--since` is set — the
//! aggregate path always anchors at the running binary's version.

use anyhow::Result;

use crate::cmd::release_notes;
use crate::cmd::update::CURRENT_VERSION;

pub fn run(version: Option<String>, since: Option<String>) -> Result<()> {
    if let Some(from) = since {
        let from = from.trim_start_matches('v').to_string();
        release_notes::print_since(&from, CURRENT_VERSION);
        return Ok(());
    }
    let v = version
        .as_deref()
        .map(|s| s.trim_start_matches('v').to_string())
        .unwrap_or_else(|| CURRENT_VERSION.to_string());
    release_notes::print_for(&v);
    Ok(())
}
