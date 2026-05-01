//! `teamctl ui` — friendly wrapper that hands off to the standalone
//! `teamctl-ui` binary.
//!
//! `teamctl-ui` ships out-of-band (separate crate, separate install) so the
//! main `teamctl` binary stays small and ratatui-free. This subcommand is
//! the discoverable entry point: if the binary is on `$PATH`, exec to it
//! transparently; if not, print a copy-paste install hint and (for
//! interactive shells) prompt to run `cargo install teamctl-ui` now.
//!
//! Friendly without auto-mutating: install never runs without an explicit
//! `y`. `--no-prompt` is the no-prompt variant of *no*, never of *yes*.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

const UI_BIN: &str = "teamctl-ui";
const INSTALL_CMD: &str = "cargo install teamctl-ui";

/// All side effects this command can perform. Pulled into a trait so the
/// argv-forwarding contract, the prompt flow, and the install-side-effect
/// gate are all unit-testable without touching the real `$PATH`, stdin,
/// or process table.
pub trait UiHost {
    /// Resolve `teamctl-ui` on `$PATH`. `None` means not installed.
    fn find_ui(&self) -> Option<PathBuf>;
    /// `true` when stdin is a TTY. `false` in pipelines / CI — the
    /// caller treats that as implicit `--no-prompt`.
    fn stdin_is_tty(&self) -> bool;
    /// Read a y/N answer from stdin. Only called when prompting is in
    /// scope (interactive + not `--no-prompt`).
    fn prompt_yes_no(&self, question: &str) -> Result<bool>;
    /// Run `cargo install teamctl-ui` synchronously, returning its exit
    /// status. Only called after an explicit `y`.
    fn run_install(&self) -> Result<()>;
    /// Hand control to `teamctl-ui` with the forwarded argv.
    ///
    /// On Unix this `exec`s and only returns on error. On Windows we
    /// spawn + wait + propagate the child exit code (the OS has no
    /// `execvp` equivalent that replaces the running process). Don't
    /// try to "unify" these — the platform divergence is load-bearing
    /// for clean handoff (no double-print on Unix, no orphaned
    /// teamctl process on Windows).
    fn exec_ui(&self, bin: PathBuf, args: Vec<String>) -> Result<()>;
}

pub fn run(no_prompt: bool, argv: Vec<String>) -> Result<()> {
    let host = RealHost;
    run_with(&host, no_prompt, argv)
}

pub fn run_with(host: &dyn UiHost, no_prompt: bool, argv: Vec<String>) -> Result<()> {
    if let Some(bin) = host.find_ui() {
        return host.exec_ui(bin, argv);
    }

    print_install_hint();

    // Non-interactive stdin (CI, pipelines) auto-treats as --no-prompt
    // per ticket acceptance. The hint above is enough — never block on
    // a prompt that no-one will answer.
    if no_prompt || !host.stdin_is_tty() {
        return Ok(());
    }

    if !host.prompt_yes_no("Install now? [y/N] ")? {
        return Ok(());
    }
    host.run_install().context("install teamctl-ui via cargo")?;
    Ok(())
}

fn print_install_hint() {
    eprintln!("teamctl-ui is not installed.");
    eprintln!();
    eprintln!("Install it with:");
    eprintln!("  {INSTALL_CMD}");
}

struct RealHost;

impl UiHost for RealHost {
    fn find_ui(&self) -> Option<PathBuf> {
        which_on_path(UI_BIN)
    }

    fn stdin_is_tty(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    }

    fn prompt_yes_no(&self, question: &str) -> Result<bool> {
        use std::io::{stdin, stdout, Write};
        let mut stdout = stdout();
        write!(stdout, "{question}").ok();
        stdout.flush().ok();
        let mut line = String::new();
        stdin()
            .read_line(&mut line)
            .context("read prompt response")?;
        let trimmed = line.trim();
        Ok(matches!(trimmed, "y" | "Y" | "yes" | "Yes" | "YES"))
    }

    fn run_install(&self) -> Result<()> {
        let status = Command::new("cargo")
            .args(["install", "teamctl-ui"])
            .status()
            .context("spawn `cargo install teamctl-ui`")?;
        if !status.success() {
            bail!("`{INSTALL_CMD}` failed with status {status}");
        }
        Ok(())
    }

    #[cfg(unix)]
    fn exec_ui(&self, bin: PathBuf, args: Vec<String>) -> Result<()> {
        use std::os::unix::process::CommandExt;
        let err = Command::new(&bin).args(&args).exec();
        Err(anyhow::Error::new(err).context(format!("exec {}", bin.display())))
    }

    #[cfg(not(unix))]
    fn exec_ui(&self, bin: PathBuf, args: Vec<String>) -> Result<()> {
        let status = Command::new(&bin)
            .args(&args)
            .status()
            .with_context(|| format!("spawn {}", bin.display()))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn which_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        format!("{bin}.exe")
    } else {
        bin.to_string()
    };
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(&exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::cell::RefCell;

    /// Recorder + scripted-answers UiHost. Enough surface to pin every
    /// branch of `run_with` without touching the real environment.
    pub struct MockHost {
        pub ui_path: Option<PathBuf>,
        pub stdin_tty: bool,
        pub prompt_answer: bool,
        pub install_result: RefCell<Result<()>>,
        pub exec_calls: RefCell<Vec<(PathBuf, Vec<String>)>>,
        pub prompt_calls: RefCell<u32>,
        pub install_calls: RefCell<u32>,
    }

    impl MockHost {
        pub fn new() -> Self {
            Self {
                ui_path: None,
                stdin_tty: true,
                prompt_answer: false,
                install_result: RefCell::new(Ok(())),
                exec_calls: RefCell::new(Vec::new()),
                prompt_calls: RefCell::new(0),
                install_calls: RefCell::new(0),
            }
        }
        pub fn with_ui_at(mut self, path: &str) -> Self {
            self.ui_path = Some(PathBuf::from(path));
            self
        }
        pub fn with_tty(mut self, tty: bool) -> Self {
            self.stdin_tty = tty;
            self
        }
        pub fn with_prompt_answer(mut self, ans: bool) -> Self {
            self.prompt_answer = ans;
            self
        }
    }

    impl UiHost for MockHost {
        fn find_ui(&self) -> Option<PathBuf> {
            self.ui_path.clone()
        }
        fn stdin_is_tty(&self) -> bool {
            self.stdin_tty
        }
        fn prompt_yes_no(&self, _q: &str) -> Result<bool> {
            *self.prompt_calls.borrow_mut() += 1;
            Ok(self.prompt_answer)
        }
        fn run_install(&self) -> Result<()> {
            *self.install_calls.borrow_mut() += 1;
            // Re-create the recorded result so the mock can be reused
            // without the cell being consumed.
            match &*self.install_result.borrow() {
                Ok(()) => Ok(()),
                Err(e) => bail!("{e}"),
            }
        }
        fn exec_ui(&self, bin: PathBuf, args: Vec<String>) -> Result<()> {
            self.exec_calls.borrow_mut().push((bin, args));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::MockHost;
    use super::*;

    #[test]
    fn execs_ui_when_found_and_forwards_argv() {
        let host = MockHost::new().with_ui_at("/usr/local/bin/teamctl-ui");
        let argv = vec!["--root".into(), "/tmp/team".into()];

        run_with(&host, false, argv.clone()).unwrap();

        let calls = host.exec_calls.borrow();
        assert_eq!(calls.len(), 1, "found-on-path → single exec");
        assert_eq!(calls[0].0, PathBuf::from("/usr/local/bin/teamctl-ui"));
        assert_eq!(calls[0].1, argv, "argv must forward verbatim");
        assert_eq!(*host.prompt_calls.borrow(), 0, "no prompt when execing");
        assert_eq!(*host.install_calls.borrow(), 0);
    }

    #[test]
    fn forwards_help_flag_to_ui_binary() {
        // Pin the most-likely first invocation a confused operator
        // tries: `teamctl ui --help`. Must reach `teamctl-ui --help`,
        // not the main binary's own help text.
        let host = MockHost::new().with_ui_at("/usr/local/bin/teamctl-ui");
        run_with(&host, false, vec!["--help".into()]).unwrap();
        let calls = host.exec_calls.borrow();
        assert_eq!(calls[0].1, vec!["--help".to_string()]);
    }

    #[test]
    fn no_ui_with_no_prompt_flag_just_hints_and_exits() {
        let host = MockHost::new().with_tty(true);
        run_with(&host, true, vec![]).unwrap();
        assert_eq!(
            *host.prompt_calls.borrow(),
            0,
            "--no-prompt suppresses tty prompt"
        );
        assert_eq!(*host.install_calls.borrow(), 0);
        assert!(host.exec_calls.borrow().is_empty());
    }

    #[test]
    fn no_ui_with_non_interactive_stdin_skips_prompt_implicitly() {
        // CI / pipeline path: stdin not a tty → prompt would block on
        // input no-one will provide. Treat as implicit --no-prompt.
        let host = MockHost::new().with_tty(false);
        run_with(&host, false, vec![]).unwrap();
        assert_eq!(*host.prompt_calls.borrow(), 0);
        assert_eq!(*host.install_calls.borrow(), 0);
    }

    #[test]
    fn no_ui_interactive_y_runs_install() {
        let host = MockHost::new().with_tty(true).with_prompt_answer(true);
        run_with(&host, false, vec![]).unwrap();
        assert_eq!(*host.prompt_calls.borrow(), 1);
        assert_eq!(*host.install_calls.borrow(), 1, "y must trigger install");
    }

    #[test]
    fn no_ui_interactive_n_exits_without_install() {
        let host = MockHost::new().with_tty(true).with_prompt_answer(false);
        run_with(&host, false, vec![]).unwrap();
        assert_eq!(*host.prompt_calls.borrow(), 1);
        assert_eq!(
            *host.install_calls.borrow(),
            0,
            "n must NOT install; friendly without auto-mutating"
        );
    }
}
