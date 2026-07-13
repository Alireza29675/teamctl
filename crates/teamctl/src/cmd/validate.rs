use std::path::Path;

use anyhow::{bail, Result};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    // Capability mismatches (e.g. `hooks:` on a codex agent) are
    // non-fatal: print them but never flip the exit code.
    for w in team_core::validate::validate_warnings(&compose) {
        eprintln!("warning: {w}");
    }
    if errs.is_empty() {
        println!(
            "ok · {} project{} · {} agent session{}",
            compose.projects.len(),
            if compose.projects.len() == 1 { "" } else { "s" },
            compose.agents().count(),
            if compose.agents().count() == 1 {
                ""
            } else {
                "s"
            },
        );
        return Ok(());
    }
    for e in &errs {
        eprintln!("error: {e}");
    }
    bail!("{} validation error(s)", errs.len());
}
