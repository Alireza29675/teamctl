//! Capture adapter commands so subprocess output cannot overwrite the TUI.

use std::process::Command;

use anyhow::{Context, Result};

pub(crate) fn run(command: &mut Command, context: &str) -> Result<()> {
    let output = command.output().with_context(|| context.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{context} exited {}: {}", output.status, detail.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_success_output_and_reports_failure_stderr() {
        run(
            Command::new("sh").args([
                "-c",
                "printf 'captured stdout'; printf 'captured stderr' >&2",
            ]),
            "adapter",
        )
        .unwrap();
        let error = run(
            Command::new("sh").args([
                "-c",
                "printf 'stdout payload'; printf 'useful error' >&2; exit 7",
            ]),
            "adapter",
        )
        .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("useful error"));
        assert!(!text.contains("stdout payload"));
    }
}
