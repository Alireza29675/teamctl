//! Process supervision.
//!
//! The default back-end is a portable `TmuxSupervisor` that works on macOS
//! and Linux. `SystemdSupervisor` and `LaunchdSupervisor` plug in behind
//! the same trait when the host supports them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

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

/// Outcome of a graceful drain. `Graceful` means the agent observed
/// `Stopped` before the timeout elapsed; `TimedOutKilled` means the
/// poll fell through and `down()` was used as a hard stop. Surfaced
/// to the caller so reload can annotate which agents were forcibly
/// killed — operator signal that a drain budget needs tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainOutcome {
    Graceful,
    TimedOutKilled,
}

pub trait Supervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()>;
    fn down(&self, spec: &AgentSpec) -> Result<()>;
    fn state(&self, spec: &AgentSpec) -> Result<AgentState>;

    /// Stop an agent gracefully. The default implementation falls
    /// back to `down()` for back-ends that don't implement signal
    /// delivery (or where graceful shutdown isn't meaningful — e.g.
    /// a `MockSupervisor` in tests).
    fn drain(&self, spec: &AgentSpec, _timeout: Duration) -> Result<DrainOutcome> {
        self.down(spec)?;
        Ok(DrainOutcome::TimedOutKilled)
    }
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

    /// Send Ctrl-C to the pane (kernel delivers SIGINT to the
    /// foreground process), then poll for `Stopped` up to `timeout`.
    /// Falls through to `kill-session` if the agent doesn't exit in
    /// time. Used by `reload` so in-flight tool calls and partial
    /// assistant responses get a chance to flush instead of being
    /// SIGKILL'd by the prior `down()`.
    fn drain(&self, spec: &AgentSpec, timeout: Duration) -> Result<DrainOutcome> {
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", &spec.tmux_session, "C-c"])
            .status();
        let outcome = poll_for_stopped(timeout, POLL_INTERVAL, || {
            self.state(spec).unwrap_or(AgentState::Unknown)
        });
        if outcome == DrainOutcome::TimedOutKilled {
            self.down(spec)?;
        }
        Ok(outcome)
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Poll `observe_state` every `interval` for up to `timeout`, returning
/// `Graceful` if `Stopped` is observed in time and `TimedOutKilled`
/// otherwise. Pulled out as a free function so it can be tested with
/// fake observers — neither tmux nor real time is involved.
fn poll_for_stopped<F: FnMut() -> AgentState>(
    timeout: Duration,
    interval: Duration,
    mut observe_state: F,
) -> DrainOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        if observe_state() == AgentState::Stopped {
            return DrainOutcome::Graceful;
        }
        if Instant::now() >= deadline {
            return DrainOutcome::TimedOutKilled;
        }
        thread::sleep(interval);
    }
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn poll_returns_graceful_when_stopped_observed_in_time() {
        let calls = RefCell::new(0u32);
        let outcome = poll_for_stopped(Duration::from_millis(50), Duration::from_millis(1), || {
            let mut n = calls.borrow_mut();
            *n += 1;
            if *n >= 2 {
                AgentState::Stopped
            } else {
                AgentState::Running
            }
        });
        assert_eq!(outcome, DrainOutcome::Graceful);
    }

    #[test]
    fn poll_falls_through_to_kill_when_agent_never_stops() {
        let outcome = poll_for_stopped(Duration::from_millis(8), Duration::from_millis(2), || {
            AgentState::Running
        });
        assert_eq!(outcome, DrainOutcome::TimedOutKilled);
    }

    #[test]
    fn poll_zero_timeout_only_checks_once_then_kills() {
        let mut calls: u32 = 0;
        let outcome = poll_for_stopped(Duration::from_millis(0), Duration::from_millis(1), || {
            calls += 1;
            AgentState::Running
        });
        assert_eq!(outcome, DrainOutcome::TimedOutKilled);
        assert_eq!(calls, 1, "single state observation before timeout");
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
