//! Process supervision.
//!
//! The default back-end is a portable `TmuxSupervisor` that works on macOS
//! and Linux. `SystemdSupervisor` and `LaunchdSupervisor` plug in behind
//! the same trait when the host supports them.

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

/// Read the per-agent env file into a list of `KEY=VALUE` assignment
/// tokens, parsed in Rust so values never reach the shell unquoted.
///
/// The file is the line-based `KEY=VALUE` shape written by
/// [`crate::render`]. Lines are taken verbatim (value bytes — spaces,
/// glob metacharacters, `$` — preserved exactly); blank lines and any
/// line without `=` are skipped. Before T-194 a no-`=` line would have
/// been word-split into command position and broken the launch
/// entirely, so skipping it is strictly safer and leaves the
/// common-case contract intact. A missing env file yields no
/// assignments — matching the prior `$(cat <missing>)` behaviour
/// (empty substitution, agent still launches); making it fatal here
/// would be a behaviour change outside this ticket's scope.
fn env_assignments(env_file: &Path) -> Result<Vec<String>> {
    let body = match std::fs::read_to_string(env_file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read env file {}", env_file.display())),
    };
    Ok(body
        .lines()
        .filter(|l| !l.trim().is_empty() && l.contains('='))
        .map(str::to_string)
        .collect())
}

/// Build the `sh -c` command line that launches one agent.
///
/// Each env assignment is read in Rust and passed to `env` as its own
/// single-quoted token, so the shell performs no word-splitting or
/// pathname expansion on env values (T-194). Command shape is otherwise
/// unchanged from the pre-T-194 form.
fn build_up_command(spec: &AgentSpec) -> Result<String> {
    let mut parts: Vec<String> = vec!["env".to_string()];
    for kv in env_assignments(&spec.env_file)? {
        parts.push(shlex::try_quote(&kv)?);
    }
    parts.push(shlex::try_quote(&spec.wrapper.display().to_string())?);
    parts.push(format!("{}:{}", spec.project, spec.agent));
    Ok(parts.join(" "))
}

/// Portable supervisor: one detached `tmux` session per agent.
pub struct TmuxSupervisor;

impl Supervisor for TmuxSupervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()> {
        if matches!(self.state(spec)?, AgentState::Running) {
            return Ok(());
        }
        let cmd = build_up_command(spec)?;
        // -x/-y set the size of the detached pane. Without these, tmux
        // falls back to 80x24 for off-screen windows, which is what the
        // child's PTY inherits via TIOCGWINSZ on stdin. We pick a size
        // larger than any common terminal so the inner TUI starts roomy;
        // once a client attaches, SIGWINCH propagates the real size
        // through `rl-watch`.
        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-x",
                "200",
                "-y",
                "50",
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
        // Tag the session so `teamctl sessions` can identify it as
        // teamctl-managed across projects without parsing the name.
        // Best-effort — `-q` swallows tmux errors so a stale tmux build
        // can't break `up`.
        let cwd_str = spec.cwd.to_string_lossy();
        for (key, value) in [
            ("@teamctl", "1"),
            ("@teamctl-project", spec.project.as_str()),
            ("@teamctl-agent", spec.agent.as_str()),
            ("@teamctl-root", cwd_str.as_ref()),
        ] {
            let _ = Command::new("tmux")
                .args(["set-option", "-q", "-t", &spec.tmux_session, key, value])
                .status();
        }
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

pub(crate) mod shlex {
    /// Minimal POSIX shell single-quote escaper so we don't pull a full dep.
    /// Shared crate-internally — `render` uses it to quote the #428 heartbeat
    /// marker path in the activity hooks.
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

#[cfg(test)]
#[cfg(unix)]
mod env_harden_tests {
    //! T-194: env-file values must reach the agent process verbatim —
    //! no shell word-splitting, no glob expansion against the cwd.
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn spec_with(env_file: &Path, wrapper: &Path, cwd: &Path) -> AgentSpec {
        AgentSpec {
            project: "proj".into(),
            agent: "agt".into(),
            tmux_session: "proj-agt".into(),
            wrapper: wrapper.to_path_buf(),
            cwd: cwd.to_path_buf(),
            env_file: env_file.to_path_buf(),
        }
    }

    /// The end-to-end Done-when pin: a value with spaces and a value
    /// with glob metacharacters must survive `sh -c <cmd>` into the
    /// launched process unchanged, even when cwd holds files a glob
    /// would otherwise match.
    #[test]
    fn env_values_round_trip_through_real_shell() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("agt.env");
        fs::write(
            &env_file,
            "MY_PATH=/some path with spaces/x\nGL=*?\nPLAIN=ok\n",
        )
        .unwrap();

        // cwd holds a decoy file: under the pre-T-194 `$(cat)` form the
        // unquoted `*?` would glob-expand to this name.
        let cwd = tempfile::tempdir().unwrap();
        fs::write(cwd.path().join("decoy"), "x").unwrap();

        let wrapper = dir.path().join("wrapper.sh");
        fs::write(
            &wrapper,
            "#!/bin/sh\nprintf 'MY_PATH=[%s]\\n' \"$MY_PATH\"\n\
             printf 'GL=[%s]\\n' \"$GL\"\nprintf 'PLAIN=[%s]\\n' \"$PLAIN\"\n",
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

        let spec = spec_with(&env_file, &wrapper, cwd.path());
        let cmd = build_up_command(&spec).unwrap();

        let out = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .current_dir(cwd.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert!(
            stdout.contains("MY_PATH=[/some path with spaces/x]"),
            "spaced value mangled by the shell — cmd: {cmd}\nstdout: {stdout}"
        );
        assert!(
            stdout.contains("GL=[*?]"),
            "glob value expanded against cwd — cmd: {cmd}\nstdout: {stdout}"
        );
        assert!(
            stdout.contains("PLAIN=[ok]"),
            "common-case value lost — cmd: {cmd}\nstdout: {stdout}"
        );
    }

    #[test]
    fn env_assignments_keeps_lines_verbatim_skips_blank_and_no_eq() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.env");
        // Spaced value, glob value, `=` inside value, empty value,
        // a blank line, and a stray no-`=` line.
        fs::write(&f, "K=v\nSP=/a b/c\nGL=*?\nEQ=a=b\nEMPTY=\n\nstray-no-eq\n").unwrap();
        assert_eq!(
            env_assignments(&f).unwrap(),
            vec![
                "K=v".to_string(),
                "SP=/a b/c".to_string(),
                "GL=*?".to_string(),
                "EQ=a=b".to_string(),
                "EMPTY=".to_string(),
            ]
        );
    }

    #[test]
    fn env_assignments_missing_file_is_empty_not_error() {
        let p = Path::new("/no/such/teamctl/env/file.env");
        assert_eq!(env_assignments(p).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn build_up_command_quotes_each_token_and_has_no_cat() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("a.env");
        fs::write(&env_file, "SP=/a b/c\nGL=*?\n").unwrap();
        let spec = spec_with(&env_file, Path::new("/w/wrap.sh"), Path::new("/tmp"));

        let cmd = build_up_command(&spec).unwrap();

        assert!(!cmd.contains("$(cat"), "command substitution gone: {cmd}");
        assert!(!cmd.contains("cat "), "no cat at all: {cmd}");
        assert!(
            cmd.contains("'SP=/a b/c'"),
            "spaced kv single-quoted: {cmd}"
        );
        assert!(cmd.contains("'GL=*?'"), "glob kv single-quoted: {cmd}");
        assert!(cmd.contains("'/w/wrap.sh'"), "wrapper still quoted: {cmd}");
        assert!(cmd.ends_with(" proj:agt"), "agent arg unchanged: {cmd}");
    }

    #[test]
    fn build_up_command_empty_env_has_no_stray_token() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("empty.env");
        fs::write(&env_file, "").unwrap();
        let spec = spec_with(&env_file, Path::new("/w/wrap.sh"), Path::new("/tmp"));

        assert_eq!(
            build_up_command(&spec).unwrap(),
            "env '/w/wrap.sh' proj:agt"
        );
    }
}
