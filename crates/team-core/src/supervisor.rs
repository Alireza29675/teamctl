//! Process supervision.
//!
//! The default back-end is a portable `TmuxSupervisor` that works on macOS
//! and Linux. `SystemdSupervisor` and `LaunchdSupervisor` plug in behind
//! the same trait when the host supports them.

use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::compose::{AgentHandle, Compose};

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
    /// Build an [`AgentSpec`] for `h` against `compose`. The agent's
    /// `cwd` is resolved via [`Compose::resolve_agent_cwd`] — typically
    /// the per-session worktree path (default since v2-A) or the
    /// project's shared `cwd` when isolation is off.
    pub fn from_handle(h: AgentHandle<'_>, compose: &Compose) -> Self {
        let cwd = compose.resolve_agent_cwd(&h);
        Self {
            project: h.project.into(),
            agent: h.agent.into(),
            tmux_session: format!(
                "{}{}-{}",
                compose.global.supervisor.tmux_prefix, h.project, h.agent
            ),
            wrapper: compose.root.join("bin/agent-wrapper.sh"),
            cwd,
            env_file: crate::render::env_path(&compose.root, h.project, h.agent),
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

    /// Cadence at which `drain` polls for `Stopped` after the
    /// graceful-stop signal is sent. Default 250ms — fine on every
    /// host we've tested. The hook exists so tests can inject a
    /// shorter cadence (no real-time waits) without going through
    /// the OS, and so a future slow-tmux host has an escape valve
    /// without forking the orchestration.
    fn drain_poll_interval(&self) -> Duration {
        Duration::from_millis(250)
    }
}

/// Generic graceful-drain orchestration used by `Supervisor` impls
/// that have a "signal a graceful stop" primitive (e.g. tmux's
/// `send-keys C-c`). Calls `signal_fn`, polls
/// `supervisor.state(spec)` for `Stopped` up to `timeout` at the
/// supervisor's `drain_poll_interval`, falls through to
/// `supervisor.down(spec)` if the agent doesn't exit in time.
///
/// Pulled out so the orchestration contract is testable end-to-end
/// against a `MockSupervisor` without a real tmux runtime.
pub fn orchestrate_drain<S, F>(
    supervisor: &S,
    spec: &AgentSpec,
    timeout: Duration,
    signal_fn: F,
) -> Result<DrainOutcome>
where
    S: Supervisor + ?Sized,
    F: FnOnce(),
{
    signal_fn();
    let outcome = poll_for_stopped(timeout, supervisor.drain_poll_interval(), || {
        supervisor.state(spec).unwrap_or(AgentState::Unknown)
    });
    if outcome == DrainOutcome::TimedOutKilled {
        supervisor.down(spec)?;
    }
    Ok(outcome)
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
        orchestrate_drain(self, spec, timeout, || {
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &spec.tmux_session, "C-c"])
                .status();
        })
    }
}

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

    /// Test supervisor that records every up/down/state/drain
    /// call, optionally returns `Stopped` after N state observations,
    /// and exposes a tunable `drain_poll_interval` so tests don't
    /// wait on real time. Every invariant a Supervisor impl is
    /// supposed to honour can be asserted against this.
    #[derive(Default)]
    struct MockSupervisor {
        calls: RefCell<Vec<&'static str>>,
        /// On the Nth state() call (1-indexed), return Stopped. 0 =
        /// always Running.
        stop_after: u32,
        state_calls: RefCell<u32>,
        poll_interval: Duration,
    }

    impl MockSupervisor {
        fn record(&self, op: &'static str) {
            self.calls.borrow_mut().push(op);
        }
    }

    impl Supervisor for MockSupervisor {
        fn up(&self, _spec: &AgentSpec) -> Result<()> {
            self.record("up");
            Ok(())
        }
        fn down(&self, _spec: &AgentSpec) -> Result<()> {
            self.record("down");
            Ok(())
        }
        fn state(&self, _spec: &AgentSpec) -> Result<AgentState> {
            self.record("state");
            let mut n = self.state_calls.borrow_mut();
            *n += 1;
            if self.stop_after > 0 && *n >= self.stop_after {
                Ok(AgentState::Stopped)
            } else {
                Ok(AgentState::Running)
            }
        }
        fn drain_poll_interval(&self) -> Duration {
            self.poll_interval
        }
    }

    fn fake_spec() -> AgentSpec {
        AgentSpec {
            project: "p".into(),
            agent: "a".into(),
            tmux_session: "p-a".into(),
            wrapper: PathBuf::from("/dev/null"),
            cwd: PathBuf::from("/tmp"),
            env_file: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn drain_with_zero_timeout_returns_timed_out_killed_and_calls_down() {
        // Contract: timeout=0 → instant signal-fn invocation, single
        // state observation, fall-through to down(). No graceful path,
        // no double-kill, no other side effects.
        let mock = MockSupervisor {
            poll_interval: Duration::from_millis(1),
            ..Default::default()
        };
        let spec = fake_spec();
        let signaled = RefCell::new(false);

        let outcome = orchestrate_drain(&mock, &spec, Duration::ZERO, || {
            *signaled.borrow_mut() = true;
        })
        .unwrap();

        assert_eq!(outcome, DrainOutcome::TimedOutKilled);
        assert!(*signaled.borrow(), "signal_fn must run before the poll");
        assert_eq!(
            mock.calls.borrow().as_slice(),
            &["state", "down"],
            "zero-timeout: one state observation then kill"
        );
    }

    #[test]
    fn drain_with_graceful_stop_does_not_call_down() {
        // Contract: agent observed `Stopped` within timeout → no
        // fall-through kill. The down() side effect is reserved for
        // forced terminations.
        let mock = MockSupervisor {
            poll_interval: Duration::from_millis(1),
            stop_after: 2, // Stopped on 2nd state() call.
            ..Default::default()
        };
        let spec = fake_spec();

        let outcome = orchestrate_drain(&mock, &spec, Duration::from_millis(100), || {}).unwrap();

        assert_eq!(outcome, DrainOutcome::Graceful);
        assert!(
            !mock.calls.borrow().contains(&"down"),
            "graceful drain must not call down(); calls: {:?}",
            mock.calls.borrow()
        );
    }

    #[test]
    fn drain_poll_interval_default_is_250ms() {
        // Pin the documented default so a future "tighten the
        // default" change has to update the docstring + this test
        // together.
        struct Default250;
        impl Supervisor for Default250 {
            fn up(&self, _: &AgentSpec) -> Result<()> {
                Ok(())
            }
            fn down(&self, _: &AgentSpec) -> Result<()> {
                Ok(())
            }
            fn state(&self, _: &AgentSpec) -> Result<AgentState> {
                Ok(AgentState::Stopped)
            }
        }
        assert_eq!(Default250.drain_poll_interval(), Duration::from_millis(250));
    }

    #[test]
    fn drain_poll_interval_override_is_used_by_orchestrator() {
        // Sanity check that the trait method's value flows into
        // poll_for_stopped — without this, a host-specific override
        // would silently no-op.
        let mock = MockSupervisor {
            poll_interval: Duration::from_millis(2),
            stop_after: 0,
            ..Default::default()
        };
        let spec = fake_spec();

        let start = Instant::now();
        let _ = orchestrate_drain(&mock, &spec, Duration::from_millis(8), || {});
        let elapsed = start.elapsed();

        // With a 2ms poll interval and an 8ms timeout, we expect a
        // handful of state observations, not 0 and not 100. Loose
        // bound — enough to catch a 250ms default leaking in.
        let states = mock
            .calls
            .borrow()
            .iter()
            .filter(|c| **c == "state")
            .count();
        assert!(
            states >= 2,
            "expected several state observations at 2ms cadence, got {states}"
        );
        assert!(
            elapsed < Duration::from_millis(60),
            "drain with 2ms interval finished too slowly ({elapsed:?})"
        );
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
