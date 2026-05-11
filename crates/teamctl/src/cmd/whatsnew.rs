//! `teamctl whatsnew [version]` — ad-hoc lookup of the GitHub release
//! body for a version, rendered the same way `teamctl update`'s
//! post-success display does it. No version → the running binary's
//! version. Non-conforming body → raw display; fetch failure → quiet
//! fallback link line, exit 0.

use anyhow::Result;

use crate::cmd::release_notes;
use crate::cmd::update::CURRENT_VERSION;

pub fn run(version: Option<String>) -> Result<()> {
    let v = version
        .as_deref()
        .map(|s| s.trim_start_matches('v').to_string())
        .unwrap_or_else(|| CURRENT_VERSION.to_string());
    match release_notes::fetch_release_body(&v) {
        Ok(body) => {
            print!("{}", release_notes::render(&v, &body));
        }
        Err(_) => {
            println!("{}", release_notes::fallback_link(&v));
        }
    }
    Ok(())
}
