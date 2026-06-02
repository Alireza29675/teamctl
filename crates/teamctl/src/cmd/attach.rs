//! `teamctl attach <agent>` — attach to the agent's tmux session.
//!
//! Read-write by default: keystrokes go to the live agent. Pass `--ro`
//! to attach read-only (observe without sending input).

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, target: &str, ro: bool) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let spec = AgentSpec::from_handle(
        handle,
        &compose.root,
        &compose.global.supervisor.tmux_prefix,
    );
    if TmuxSupervisor.state(&spec)? != AgentState::Running {
        bail!(
            "agent {target} is not running (tmux session {} absent). Run `teamctl up`.",
            spec.tmux_session
        );
    }
    // `-r` makes tmux attach read-only; without it, keystrokes reach the agent.
    let mut args = vec!["attach-session", "-t", spec.tmux_session.as_str()];
    if ro {
        args.insert(1, "-r");
    }
    let st = Command::new("tmux").args(&args).status()?;
    anyhow::ensure!(st.success(), "tmux attach exited {st}");
    Ok(())
}
