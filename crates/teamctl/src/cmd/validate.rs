use std::path::Path;

use anyhow::{bail, Result};

pub fn run(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let errs = team_core::validate::validate(&compose);
    let warns = team_core::validate::warnings(&compose);
    for w in &warns {
        eprintln!("warn · {w}");
    }
    if errs.is_empty() {
        println!(
            "ok · {} project{} · {} agent{}",
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
