//! `teamctl adjust` — friendly CLI handle that confirms intent and
//! hands off to the `/teamctl:adjust` skill inside Claude Code.
//!
//! Thin shim. Mirrors `cmd/ui.rs`'s trait-based test surface so the
//! argv handed to `claude` can be pinned without spawning a real
//! process; the exec mechanics match `init.rs`'s `exec_guided` for
//! behavioural parity between `teamctl init --template guided` and
//! `teamctl adjust`.
//!
//! Defaults to **No** on Enter. `--yes` is rejected because the skill
//! itself is interactive — accepting `--yes` would just defer the
//! surprise to the picker inside Claude Code (#206 Q3 stance).

use std::process::Command;

use anyhow::{bail, Context, Result};

/// Single positional we hand to `claude`. Lifted out so the unit test
/// can assert the argv shape against the same constant the runtime
/// uses, not a hand-typed string in two places.
const SKILL_ARG: &str = "/teamctl:adjust";

/// All side effects this command can perform. Behind a trait so the
/// prompt branch + the argv-forwarding contract are unit-testable
/// without touching the real stdin or process table. Same shape as
/// `cmd/ui.rs`'s `UiHost`, minus the install / TTY-detection rungs
/// (no install path, no `--no-prompt` flag — there's nothing this
/// command can `cargo install`).
pub trait AdjustHost {
    /// Read a y/N answer. Caller wraps in the prompt text.
    fn prompt_yes_no(&self, question: &str) -> Result<bool>;
    /// Hand control to `claude` with the forwarded argv.
    fn exec_claude(&self, args: &[&str]) -> Result<()>;
}

pub fn run(yes: bool) -> Result<()> {
    // `--yes` would skip the only meaningful gate this CLI has. The
    // skill behind it is interactive top-to-bottom; auto-accepting
    // here just shifts the surprise into Claude Code. Reject early
    // with a clear message rather than letting it slip past, matching
    // `cmd/init.rs`'s `--template guided --yes` rejection.
    if yes {
        bail!(
            "`teamctl adjust` is interactive-only and incompatible with `--yes`. \
             Drop `--yes` to run the flow; the skill itself collects what it needs."
        );
    }
    let host = RealHost;
    run_with(&host)
}

pub fn run_with(host: &dyn AdjustHost) -> Result<()> {
    let go = host.prompt_yes_no(
        "This will open Claude Code and run `/teamctl:adjust` to help you evolve your team. \
         Continue? [y/N] ",
    )?;
    if !go {
        // Declining the prompt is not an error — ticket DoD: "If N,
        // exit cleanly." Returns Ok so the binary exits 0 without an
        // "Error: aborted" line. Deliberate divergence from
        // `cmd/init.rs`'s guided path, which bails non-zero on
        // decline; that's pre-existing behaviour on init, not the
        // pattern this ticket inherits.
        return Ok(());
    }
    host.exec_claude(&[SKILL_ARG])
}

struct RealHost;

impl AdjustHost for RealHost {
    fn prompt_yes_no(&self, question: &str) -> Result<bool> {
        use std::io::{stderr, stdin, Write};
        let mut stderr = stderr();
        write!(stderr, "{question}").ok();
        stderr.flush().ok();
        let mut line = String::new();
        stdin()
            .read_line(&mut line)
            .context("read prompt response")?;
        let s = line.trim();
        Ok(matches!(s, "y" | "Y" | "yes" | "Yes" | "YES"))
    }

    fn exec_claude(&self, args: &[&str]) -> Result<()> {
        let status = Command::new("claude")
            .args(args)
            .status()
            .with_context(|| {
                "failed to launch `claude` — is Claude Code installed and on PATH? See \
                 https://code.claude.com/docs"
            })?;
        if !status.success() {
            bail!(
                "`claude {}` exited with status {status} — see the Claude Code output above \
                 for details.",
                args.join(" ")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::cell::RefCell;

    /// Recorder + scripted-answer `AdjustHost`. Pins every branch of
    /// `run_with` without touching stdin or `$PATH`.
    pub struct MockHost {
        pub answer: bool,
        pub exec_calls: RefCell<Vec<Vec<String>>>,
        pub prompt_calls: RefCell<u32>,
    }

    impl MockHost {
        pub fn new() -> Self {
            Self {
                answer: false,
                exec_calls: RefCell::new(Vec::new()),
                prompt_calls: RefCell::new(0),
            }
        }
        pub fn with_answer(mut self, ans: bool) -> Self {
            self.answer = ans;
            self
        }
    }

    impl AdjustHost for MockHost {
        fn prompt_yes_no(&self, _q: &str) -> Result<bool> {
            *self.prompt_calls.borrow_mut() += 1;
            Ok(self.answer)
        }
        fn exec_claude(&self, args: &[&str]) -> Result<()> {
            self.exec_calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::MockHost;
    use super::*;

    #[test]
    fn y_execs_claude_with_skill_arg_only() {
        // T-241 DoD argv-shape pin: `y` → `claude /teamctl:adjust`,
        // exactly one positional. The skill collects everything else
        // interactively; nothing else is forwarded.
        let host = MockHost::new().with_answer(true);
        run_with(&host).unwrap();
        let calls = host.exec_calls.borrow();
        assert_eq!(calls.len(), 1, "single exec on accept");
        assert_eq!(
            calls[0],
            vec![SKILL_ARG.to_string()],
            "argv must be exactly the skill invocation"
        );
        assert_eq!(*host.prompt_calls.borrow(), 1);
    }

    #[test]
    fn n_exits_clean_without_exec() {
        let host = MockHost::new().with_answer(false);
        run_with(&host).expect("decline must exit cleanly, not error");
        assert!(host.exec_calls.borrow().is_empty());
        assert_eq!(*host.prompt_calls.borrow(), 1);
    }

    #[test]
    fn yes_flag_rejects_before_prompt() {
        let err = run(true).expect_err("--yes must error");
        let msg = format!("{err}");
        assert!(msg.contains("interactive-only"), "msg: {msg}");
        assert!(msg.contains("--yes"), "msg: {msg}");
    }
}
