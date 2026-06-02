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
    let st = Command::new("tmux")
        .args(attach_args(ro, &spec.tmux_session))
        .status()?;
    anyhow::ensure!(st.success(), "tmux attach exited {st}");
    Ok(())
}

/// Build the `tmux attach-session` argument vector. With `ro` the `-r`
/// flag is inserted before `-t`, giving `attach-session -r -t <session>`,
/// which makes tmux attach read-only; without it, keystrokes reach the
/// live agent.
fn attach_args(ro: bool, session: &str) -> Vec<&str> {
    let mut args = vec!["attach-session", "-t", session];
    if ro {
        args.insert(1, "-r");
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    // #385: attach is now read-write by default. Without `--ro` the
    // arg vector carries no `-r`, so tmux attaches writable and the
    // operator's keystrokes reach the live agent.
    #[test]
    fn read_write_default_omits_dash_r() {
        assert_eq!(
            attach_args(false, "t-demo-mgr"),
            ["attach-session", "-t", "t-demo-mgr"]
        );
    }

    // `--ro` inserts `-r` immediately before `-t`, yielding tmux's
    // read-only `attach-session -r -t <session>` form (observe without
    // sending input).
    #[test]
    fn ro_inserts_dash_r_before_dash_t() {
        assert_eq!(
            attach_args(true, "t-demo-mgr"),
            ["attach-session", "-r", "-t", "t-demo-mgr"]
        );
    }
}
