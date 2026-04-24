//! Process supervision.
//!
//! Phase 1 ships a portable tmux back-end that works on macOS and Linux.
//! Phase 7 adds `SystemdSupervisor` and `LaunchdSupervisor` behind the same
//! trait.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::compose::AgentHandle;

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub project: String,
    pub agent: String,
    pub tmux_session: String,
    pub wrapper: PathBuf,
    pub cwd: PathBuf,
    pub env_file: PathBuf,
}

impl AgentSpec {
    pub fn from_handle(h: AgentHandle<'_>, root: &Path, tmux_prefix: &str) -> Self {
        Self {
            project: h.project.into(),
            agent: h.agent.into(),
            tmux_session: format!("{tmux_prefix}{}-{}", h.project, h.agent),
            wrapper: root.join("bin/agent-wrapper.sh"),
            cwd: root.to_path_buf(),
            env_file: crate::render::env_path(root, h.project, h.agent),
        }
    }
}

/// Observed state of an agent's supervising process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Running,
    Stopped,
    Unknown,
}

pub trait Supervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()>;
    fn down(&self, spec: &AgentSpec) -> Result<()>;
    fn state(&self, spec: &AgentSpec) -> Result<AgentState>;
}

/// Portable supervisor: one detached `tmux` session per agent.
pub struct TmuxSupervisor;

impl Supervisor for TmuxSupervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()> {
        if matches!(self.state(spec)?, AgentState::Running) {
            return Ok(());
        }
        let cmd = format!(
            "env $(cat {env}) {wrapper} {project}:{agent}",
            env = shlex::try_quote(&spec.env_file.display().to_string())?,
            wrapper = shlex::try_quote(&spec.wrapper.display().to_string())?,
            project = spec.project,
            agent = spec.agent,
        );
        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &spec.tmux_session,
                "-c",
                &spec.cwd.display().to_string(),
                "sh",
                "-c",
                &cmd,
            ])
            .status()
            .context("spawn tmux new-session")?;
        anyhow::ensure!(status.success(), "tmux new-session exited {status}");
        Ok(())
    }

    fn down(&self, spec: &AgentSpec) -> Result<()> {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &spec.tmux_session])
            .status();
        Ok(())
    }

    fn state(&self, spec: &AgentSpec) -> Result<AgentState> {
        let out = Command::new("tmux")
            .args(["has-session", "-t", &spec.tmux_session])
            .output();
        Ok(match out {
            Ok(o) if o.status.success() => AgentState::Running,
            Ok(_) => AgentState::Stopped,
            Err(_) => AgentState::Unknown,
        })
    }
}

mod shlex {
    /// Minimal POSIX shell single-quote escaper so we don't pull a full dep.
    pub fn try_quote(s: &str) -> anyhow::Result<String> {
        anyhow::ensure!(!s.contains('\0'), "null byte in shell arg");
        let escaped = s.replace('\'', r"'\''");
        Ok(format!("'{escaped}'"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn quotes_plain_path() {
            assert_eq!(try_quote("/a/b.sh").unwrap(), "'/a/b.sh'");
        }

        #[test]
        fn escapes_embedded_single_quote() {
            assert_eq!(try_quote("x'y").unwrap(), r"'x'\''y'");
        }
    }
}
