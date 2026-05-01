//! Pane capture — abstracts how the UI reads the focused agent's
//! tmux scrollback so tests can stub it out. Production hits
//! `tmux capture-pane`; tests pass a `MockPaneSource` with canned
//! lines.
//!
//! The detail pane in PR-UI-2 polls the `PaneSource` once per
//! refresh tick (currently 1s, same cadence as the roster) and
//! re-renders. For PR-UI-3 / PR-UI-4 a streaming `tmux pipe-pane`
//! variant can implement the same trait without changing callers.

use std::process::Command;

use anyhow::{Context, Result};

/// Lookup contract: given a tmux session name, return its scrollback
/// as a list of lines. Implementations may bound the depth — the
/// production tmux variant takes the last 3000 lines via
/// `capture-pane -S -3000`, matching `teamctl logs`.
pub trait PaneSource: Send + Sync {
    fn capture(&self, session: &str) -> Result<Vec<String>>;
}

/// Production implementation — shells out to `tmux capture-pane`.
/// `-J` joins wrapped lines, `-p` writes to stdout, `-S -3000`
/// pulls the last 3000 lines of scrollback.
#[derive(Debug, Default, Clone, Copy)]
pub struct TmuxPaneSource;

impl PaneSource for TmuxPaneSource {
    fn capture(&self, session: &str) -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args(["capture-pane", "-p", "-J", "-S", "-3000", "-t", session])
            .output()
            .with_context(|| format!("invoke tmux capture-pane -t {session}"))?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect())
    }
}

/// Take the last `n` lines so the detail pane never overruns its
/// rect. Free function so tests can pin the slice without
/// constructing a widget.
pub fn tail_lines(lines: &[String], n: usize) -> Vec<String> {
    let len = lines.len();
    let start = len.saturating_sub(n);
    lines[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test stub — returns the canned lines on every call, records
    /// every session it was queried with so tests can assert that
    /// the right session got captured.
    #[derive(Default)]
    pub struct MockPaneSource {
        pub lines: Vec<String>,
        pub asked: Mutex<Vec<String>>,
    }

    impl PaneSource for MockPaneSource {
        fn capture(&self, session: &str) -> Result<Vec<String>> {
            self.asked.lock().unwrap().push(session.to_string());
            Ok(self.lines.clone())
        }
    }

    #[test]
    fn tail_lines_takes_last_n() {
        let v: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let tail = tail_lines(&v, 3);
        assert_eq!(tail, vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn tail_lines_under_n_returns_all() {
        let v = vec!["a".to_string(), "b".to_string()];
        assert_eq!(tail_lines(&v, 5), v);
    }

    #[test]
    fn tail_lines_empty_returns_empty() {
        let v: Vec<String> = Vec::new();
        assert!(tail_lines(&v, 5).is_empty());
    }

    #[test]
    fn mock_pane_source_records_session() {
        let mock = MockPaneSource {
            lines: vec!["hi".into(), "bye".into()],
            asked: Mutex::new(Vec::new()),
        };
        let lines = mock.capture("t-p-a").unwrap();
        assert_eq!(lines, vec!["hi", "bye"]);
        assert_eq!(mock.asked.lock().unwrap().clone(), vec!["t-p-a"]);
    }
}
