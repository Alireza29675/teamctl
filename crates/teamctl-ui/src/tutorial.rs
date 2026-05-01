//! First-launch detection via a marker file under the user's config
//! directory. PR-UI-1 ships only the plumbing: `is_completed()` reads
//! the marker; the actual onboarding tutorial that writes the marker
//! lands in PR-UI-7.

use std::path::PathBuf;

const MARKER_RELATIVE: &str = "teamctl/ui-tutorial-completed";

/// Returns the path the marker file lives at. Honours `XDG_CONFIG_HOME`
/// via `dirs::config_dir`, falling back to platform defaults.
pub fn marker_path() -> Option<PathBuf> {
    dirs::config_dir().map(|root| root.join(MARKER_RELATIVE))
}

/// Has the operator finished the onboarding tutorial on this machine?
/// Returns `false` when the marker is absent, the config dir is
/// unresolvable, or the file is unreadable for any reason — first-launch
/// detection should never panic an otherwise-healthy launch.
pub fn is_completed() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// Write the marker so subsequent launches skip the tutorial. Reserved
/// for PR-UI-7's tutorial completion path; here only as the boundary
/// of the public API so the rest of the crate can refer to it.
#[allow(dead_code)]
pub fn mark_completed() -> std::io::Result<()> {
    let Some(path) = marker_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"")
}
