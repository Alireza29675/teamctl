//! `teamctl attach <agent>` — attach to the agent's tmux session.
//!
//! Read-only by default. `--rw` allows input but requires retyping the
//! agent name to confirm.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

pub fn run(root: &Path, target: &str, rw: bool) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let spec = AgentSpec::from_handle(handle, &compose);
    if TmuxSupervisor.state(&spec)? != AgentState::Running {
        bail!(
            "agent {target} is not running (tmux session {} absent). Run `teamctl up`.",
            spec.tmux_session
        );
    }
    if rw {
        eprint!(
            "⚠️  Attaching read/write to {target}. Keystrokes will be sent to the live agent.\n\
             Type the agent id (`{target}`) to confirm: "
        );
        io::stderr().flush().ok();
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        if line.trim() != target {
            bail!("aborted — confirmation did not match");
        }
        let st = Command::new("tmux")
            .args(["attach-session", "-t", &spec.tmux_session])
            .status()?;
        anyhow::ensure!(st.success(), "tmux attach exited {st}");
    } else {
        // Read-only attach — tmux supports it via `-r`.
        let st = Command::new("tmux")
            .args(["attach-session", "-r", "-t", &spec.tmux_session])
            .status()?;
        anyhow::ensure!(st.success(), "tmux attach exited {st}");
    }
    Ok(())
}
