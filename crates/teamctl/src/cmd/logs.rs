use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use team_core::supervisor::{native_log_path, AgentSpec, AgentSupervisor, Supervisor};

pub fn run(root: &Path, target: &str) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let spec = AgentSpec::from_handle(
        handle,
        &compose.root,
        &compose.global.supervisor.tmux_prefix,
    );
    if handle
        .spec
        .execution
        .as_ref()
        .is_some_and(|e| e.mode == team_core::compose::ExecutionMode::Native)
    {
        let path = native_log_path(&spec);
        if !path.is_file() {
            bail!("no native log for {target}; run `teamctl up` first");
        }
        print!("{}", native_tail(&path)?);
        return Ok(());
    }
    if AgentSupervisor.state(&spec)? == team_core::supervisor::AgentState::Stopped {
        bail!(
            "agent {target} is not running (tmux session {} absent)",
            spec.tmux_session
        );
    }
    // Dump the scrollback of the tmux pane.
    let status = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",
            "-J",
            "-S",
            "-3000",
            "-t",
            &spec.tmux_session,
        ])
        .status()?;
    if !status.success() {
        bail!("tmux capture-pane exited {status}");
    }
    Ok(())
}

fn native_tail(path: &Path) -> Result<String> {
    const MAX_BYTES: u64 = 256 * 1024;
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(MAX_BYTES)))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<_> = text.lines().collect();
    let start = lines.len().saturating_sub(3000);
    Ok(format!("{}\n", lines[start..].join("\n")))
}
