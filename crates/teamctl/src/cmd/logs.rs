use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use team_core::supervisor::{AgentSpec, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, target: &str) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let spec = AgentSpec::from_handle(handle, &compose);
    if TmuxSupervisor.state(&spec)? == team_core::supervisor::AgentState::Stopped {
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
