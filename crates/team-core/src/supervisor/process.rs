//! Detached, low-idle-memory mailbox workers. Provider CLIs live only during turns.

use super::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;

fn process_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Serialize, Deserialize)]
struct ProcessIdentity {
    pid: u32,
    token: String,
    #[serde(default)]
    container: Option<String>,
}

pub fn native_process_path(spec: &AgentSpec) -> PathBuf {
    spec.cwd
        .join("state/processes")
        .join(format!("{}-{}.json", spec.project, spec.agent))
}

pub fn native_log_path(spec: &AgentSpec) -> PathBuf {
    native_process_path(spec).with_extension("log")
}

fn identity(spec: &AgentSpec) -> Result<Option<ProcessIdentity>> {
    match fs::read(native_process_path(spec)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("read native worker identity")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn is_native(spec: &AgentSpec) -> bool {
    env_assignments(&spec.env_file)
        .unwrap_or_default()
        .iter()
        .any(|line| line == "EXECUTION_MODE=native")
}

fn observed_native(spec: &AgentSpec) -> bool {
    match fs::read_to_string(native_process_path(spec).with_extension("supervisor")) {
        Ok(mode) => mode == "native",
        Err(_) => is_native(spec) || native_process_path(spec).exists(),
    }
}

fn clear_mode(spec: &AgentSpec) -> Result<()> {
    match fs::remove_file(native_process_path(spec).with_extension("supervisor")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Verify an unguessable launch token in the live process argv before signaling.
/// A stale PID file must never authorize killing a reused, unrelated PID.
fn is_owned(process: &ProcessIdentity) -> Result<bool> {
    if process.pid <= 1 || process.token.len() < 32 {
        return Ok(false);
    }
    let out = Command::new("ps")
        .args(["-p", &process.pid.to_string(), "-o", "args="])
        .output()
        .context("inspect native worker identity")?;
    if !out.status.success() {
        return Ok(false);
    }
    let argv = String::from_utf8_lossy(&out.stdout);
    Ok(argv.split_whitespace().any(|arg| arg == process.token)
        && argv.split_whitespace().any(|arg| arg == "agent-run"))
}

pub struct ProcessSupervisor;

impl Supervisor for ProcessSupervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()> {
        #[cfg(not(unix))]
        anyhow::bail!("native execution currently requires Unix process groups");
        #[cfg(unix)]
        {
            if self.state(spec)? == AgentState::Running {
                return Ok(());
            }
            if let Some(old) = identity(spec)? {
                cleanup_container(&old)?;
            }
            let path = native_process_path(spec);
            fs::create_dir_all(path.parent().context("native process directory")?)?;
            let _lock =
                crate::persistence::ProcessLock::acquire(&path.with_extension("lock.sqlite"))?;
            if self.state(spec)? == AgentState::Running {
                return Ok(());
            }
            if env_assignments(&spec.env_file)?
                .iter()
                .any(|line| line == "SANDBOX_TYPE=docker")
            {
                let info = Command::new("docker")
                    .args(["info", "--format", "{{.ServerVersion}}"])
                    .output()
                    .context("Docker sandbox requires the Docker CLI and running daemon")?;
                anyhow::ensure!(
                    info.status.success(),
                    "Docker sandbox unavailable: {}",
                    String::from_utf8_lossy(&info.stderr).trim()
                );
            }
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(native_log_path(spec))?;
            let token = process_token()?;
            let mut command =
                Command::new(std::env::current_exe().context("resolve teamctl executable")?);
            command
                .args([
                    "--root",
                    &spec.cwd.to_string_lossy(),
                    "agent-run",
                    &format!("{}:{}", spec.project, spec.agent),
                    "--worker-token",
                    &token,
                ])
                .current_dir(&spec.cwd)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and accesses no Rust shared state.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().context("start native agent worker")?;
            let sandbox_docker = env_assignments(&spec.env_file)?
                .iter()
                .any(|line| line == "SANDBOX_TYPE=docker");
            let process = ProcessIdentity {
                pid: child.id(),
                token,
                container: sandbox_docker.then(|| {
                    format!(
                        "teamctl-{}-{}-{}",
                        &blake3::hash(spec.cwd.to_string_lossy().as_bytes()).to_hex()[..16],
                        spec.project,
                        spec.agent
                    )
                }),
            };
            if let Err(error) =
                crate::persistence::atomic_write(&path, &serde_json::to_vec(&process)?)
            {
                // SAFETY: this is the process group just created by this call.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGTERM);
                }
                let _ = child.wait();
                return Err(error.into());
            }
            thread::sleep(Duration::from_millis(100));
            if let Some(status) = child.try_wait()? {
                let _ = fs::remove_file(&path);
                anyhow::bail!(
                    "native worker exited {status}; inspect {}",
                    native_log_path(spec).display()
                );
            }
            Ok(())
        }
    }

    fn down(&self, spec: &AgentSpec) -> Result<()> {
        self.drain(spec, Duration::from_secs(3)).map(|_| ())
    }

    fn state(&self, spec: &AgentSpec) -> Result<AgentState> {
        Ok(match identity(spec)? {
            Some(p) if is_owned(&p)? => AgentState::Running,
            _ => AgentState::Stopped,
        })
    }

    fn drain(&self, spec: &AgentSpec, timeout: Duration) -> Result<DrainOutcome> {
        let recorded = identity(spec)?;
        #[cfg(unix)]
        if let Some(process) = recorded.as_ref() {
            if is_owned(process)? {
                // The worker owns its session/process group; its provider subprocesses
                // inherit that group and receive the same stop signal.
                // SAFETY: verified PID belongs to our detached worker above.
                let result = unsafe { libc::kill(-(process.pid as i32), libc::SIGTERM) };
                if result == -1
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                let deadline = Instant::now() + timeout.max(Duration::from_millis(250));
                while is_owned(process)? && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(25));
                }
                if is_owned(process)? {
                    // SAFETY: identity was checked again immediately before the kill.
                    let result = unsafe { libc::kill(-(process.pid as i32), libc::SIGKILL) };
                    if result == -1
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    cleanup_container(process)?;
                    let _ = fs::remove_file(native_process_path(spec));
                    return Ok(DrainOutcome::TimedOutKilled);
                }
            }
        }
        if let Some(process) = &recorded {
            cleanup_container(process)?;
        }
        match fs::remove_file(native_process_path(spec)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(DrainOutcome::Graceful)
    }
}

fn cleanup_container(process: &ProcessIdentity) -> Result<()> {
    let Some(name) = &process.container else {
        return Ok(());
    };
    let inspect = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{index .Config.Labels \"teamctl.managed\"}}",
            name,
        ])
        .output()
        .context("inspect native agent container")?;
    if !inspect.status.success() {
        let error = String::from_utf8_lossy(&inspect.stderr);
        let lower = error.to_ascii_lowercase();
        if lower.contains("no such object") || lower.contains("no such container") {
            return Ok(());
        }
        anyhow::bail!("inspect native agent container: {}", error.trim());
    }
    anyhow::ensure!(
        String::from_utf8_lossy(&inspect.stdout).trim() == "true",
        "refusing to remove unmanaged container {name}"
    );
    let output = Command::new("docker")
        .args(["rm", "--force", name])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "remove native agent container: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Lifecycle dispatch follows desired mode on start and a live recorded worker
/// on stop, so changing modes cannot strand the old native process.
pub struct AgentSupervisor;

impl Supervisor for AgentSupervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()> {
        let native = is_native(spec);
        if native {
            ProcessSupervisor.up(spec)
        } else {
            if ProcessSupervisor.state(spec)? == AgentState::Stopped {
                let _ = fs::remove_file(native_process_path(spec));
            }
            TmuxSupervisor.up(spec)
        }?;
        crate::persistence::atomic_write(
            &native_process_path(spec).with_extension("supervisor"),
            if native { b"native" } else { b"tmux" },
        )?;
        Ok(())
    }
    fn down(&self, spec: &AgentSpec) -> Result<()> {
        let native = ProcessSupervisor.state(spec)? == AgentState::Running;
        if native || native_process_path(spec).exists() {
            ProcessSupervisor.down(spec)?;
        }
        if native || observed_native(spec) {
            clear_mode(spec)
        } else {
            TmuxSupervisor.down(spec)?;
            clear_mode(spec)
        }
    }
    fn state(&self, spec: &AgentSpec) -> Result<AgentState> {
        if ProcessSupervisor.state(spec)? == AgentState::Running {
            return Ok(AgentState::Running);
        }
        if observed_native(spec) {
            Ok(AgentState::Stopped)
        } else {
            TmuxSupervisor.state(spec)
        }
    }
    fn drain(&self, spec: &AgentSpec, timeout: Duration) -> Result<DrainOutcome> {
        let outcome =
            if ProcessSupervisor.state(spec)? == AgentState::Running || observed_native(spec) {
                ProcessSupervisor.drain(spec, timeout)
            } else {
                TmuxSupervisor.drain(spec, timeout)
            }?;
        clear_mode(spec)?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_pid_cannot_authorize_signaling_unrelated_process() {
        let process = ProcessIdentity {
            pid: std::process::id(),
            token: process_token().unwrap(),
            container: None,
        };
        assert!(!is_owned(&process).unwrap());
    }
    #[test]
    fn native_stopped_state_does_not_need_tmux() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join("agent.env");
        fs::write(&env, "EXECUTION_MODE=native\n").unwrap();
        let spec = AgentSpec {
            project: "p".into(),
            agent: "a".into(),
            tmux_session: "p-a".into(),
            wrapper: PathBuf::new(),
            cwd: dir.path().into(),
            env_file: env,
        };
        assert_eq!(AgentSupervisor.state(&spec).unwrap(), AgentState::Stopped);
        AgentSupervisor.down(&spec).unwrap();
    }
}
