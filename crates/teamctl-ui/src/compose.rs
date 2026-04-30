//! Send-mail compose modal — multi-line vim-style editor + CLI send.
//!
//! Two abstractions live here, mirroring the PR-UI-4 approvals
//! split:
//!
//! - `Editor` — pure-state vim-style multi-line buffer. `apply_key`
//!   takes a `KeyEvent` and returns an `EditorAction` so the
//!   surrounding App can react to commands like "send" / "cancel"
//!   without the editor itself knowing about the message bus.
//! - `MessageSender` — write side. `CliMessageSender` shells out to
//!   `teamctl send <agent> "<body>"` (DM) or
//!   `teamctl broadcast #<channel> "<body>"` (broadcast), the same
//!   architectural discipline as PR-UI-4's `CliApprovalDecider`. A
//!   direct `INSERT INTO messages …` from the UI would silently
//!   sidestep the channel-ACL + ratelimit + delivery hooks the CLI
//!   already runs through.
//!
//! Vim keybindings shipped in PR-UI-5: insert mode (`i`/`a`/`o`,
//! Esc back to Normal), Normal motions (`h`/`j`/`k`/`l`, arrows,
//! `0`/`$`), ex command shim (`:w`/`:q`/`:wq`), Ctrl+Enter to send,
//! Esc-Esc to cancel. Word motions (`w`/`b`) and line ops
//! (`dd`/`yy`/`p`) deferred to the PR-UI-7 polish cycle — flagged
//! in the PR description.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeTarget {
    /// DM to a specific agent. `agent_id` is `<project>:<agent>`.
    Dm {
        agent_id: String,
        project_id: String,
    },
    /// Broadcast to a channel. `channel_id` is `<project>:<name>`,
    /// rendered as `#<name>` in the modal title.
    Broadcast {
        channel_id: String,
        project_id: String,
    },
}

impl ComposeTarget {
    pub fn title(&self) -> String {
        match self {
            ComposeTarget::Dm { agent_id, .. } => format!("→ {agent_id}"),
            ComposeTarget::Broadcast { channel_id, .. } => {
                let short = channel_id
                    .rsplit_once(':')
                    .map(|(_, n)| n)
                    .unwrap_or(channel_id);
                format!("→ #{short}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    /// Awaiting an ex-command after `:`. `ex_buffer` accumulates
    /// the typed string; Enter dispatches it.
    Ex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub mode: VimMode,
    pub ex_buffer: String,
    /// Tracks whether the previous keypress was `Esc`. Two Escs in
    /// a row from any mode cancel the surrounding modal — same
    /// shape SPEC §4 specifies for "close all modals."
    pub esc_armed: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            // Compose modals open in Insert because typing is the
            // central UX — operators expect their first keystroke
            // to land in the buffer.
            mode: VimMode::Insert,
            ex_buffer: String::new(),
            esc_armed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    /// Keep the modal open; editor consumed the key.
    Continue,
    /// Operator hit `Ctrl+Enter` or `:wq` — send + close.
    Send,
    /// Operator hit Esc-Esc or `:q` — close without send.
    Cancel,
}

impl Editor {
    /// Final body for sending. Joins lines with `\n`; trailing
    /// blank lines are stripped so a single newline at the bottom
    /// doesn't sneak past the operator's intent.
    pub fn body(&self) -> String {
        let mut out = self.lines.join("\n");
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// Apply a keypress and return what the surrounding modal
    /// should do. Held under `&mut self` so tests can drive the
    /// editor deterministically without a `Terminal`.
    pub fn apply_key(&mut self, k: KeyEvent) -> EditorAction {
        if k.kind != KeyEventKind::Press {
            return EditorAction::Continue;
        }

        // Ctrl+Enter sends from any mode — send is universal.
        if k.code == KeyCode::Enter && k.modifiers.contains(KeyModifiers::CONTROL) {
            return EditorAction::Send;
        }

        // Esc-Esc handling spans modes: a single Esc out of Insert
        // / Ex arms the second-Esc; from Normal the first Esc is
        // the arming press. Any non-Esc key clears the arm.
        if k.code == KeyCode::Esc {
            return self.handle_esc();
        }
        self.esc_armed = false;

        match self.mode {
            VimMode::Insert => self.apply_insert(k),
            VimMode::Normal => self.apply_normal(k),
            VimMode::Ex => self.apply_ex(k),
        }
    }

    fn handle_esc(&mut self) -> EditorAction {
        // Two Escs in a row → cancel the modal regardless of mode.
        if self.esc_armed {
            return EditorAction::Cancel;
        }
        self.esc_armed = true;
        match self.mode {
            VimMode::Insert | VimMode::Ex => {
                self.mode = VimMode::Normal;
                self.ex_buffer.clear();
            }
            VimMode::Normal => {
                // Already Normal — Esc just arms the second-Esc.
            }
        }
        EditorAction::Continue
    }

    fn apply_insert(&mut self, k: KeyEvent) -> EditorAction {
        match k.code {
            KeyCode::Char(c) => {
                let line = &mut self.lines[self.cursor_row];
                let col = self.cursor_col.min(line.len());
                line.insert(col, c);
                self.cursor_col = col + 1;
            }
            KeyCode::Enter => {
                let line = &mut self.lines[self.cursor_row];
                let col = self.cursor_col.min(line.len());
                let tail = line.split_off(col);
                self.cursor_row += 1;
                self.lines.insert(self.cursor_row, tail);
                self.cursor_col = 0;
            }
            KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    let line = &mut self.lines[self.cursor_row];
                    let col = self.cursor_col.min(line.len());
                    line.remove(col - 1);
                    self.cursor_col = col - 1;
                } else if self.cursor_row > 0 {
                    let removed = self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    let prev_len = self.lines[self.cursor_row].len();
                    self.lines[self.cursor_row].push_str(&removed);
                    self.cursor_col = prev_len;
                }
            }
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            _ => {}
        }
        EditorAction::Continue
    }

    fn apply_normal(&mut self, k: KeyEvent) -> EditorAction {
        match k.code {
            KeyCode::Char('i') => self.mode = VimMode::Insert,
            KeyCode::Char('a') => {
                self.move_right_or_eol();
                self.mode = VimMode::Insert;
            }
            KeyCode::Char('o') => {
                self.cursor_row += 1;
                self.lines.insert(self.cursor_row, String::new());
                self.cursor_col = 0;
                self.mode = VimMode::Insert;
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_left(),
            KeyCode::Char('l') | KeyCode::Right => self.move_right(),
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('0') => self.cursor_col = 0,
            KeyCode::Char('$') => {
                self.cursor_col = self.lines[self.cursor_row].len();
            }
            KeyCode::Char(':') => {
                self.mode = VimMode::Ex;
                self.ex_buffer.clear();
            }
            _ => {}
        }
        EditorAction::Continue
    }

    fn apply_ex(&mut self, k: KeyEvent) -> EditorAction {
        match k.code {
            KeyCode::Char(c) => {
                self.ex_buffer.push(c);
                EditorAction::Continue
            }
            KeyCode::Backspace => {
                self.ex_buffer.pop();
                EditorAction::Continue
            }
            KeyCode::Enter => {
                let cmd = std::mem::take(&mut self.ex_buffer);
                self.mode = VimMode::Normal;
                match cmd.trim() {
                    "wq" | "x" => EditorAction::Send,
                    "q" | "q!" => EditorAction::Cancel,
                    "w" => EditorAction::Continue,
                    _ => EditorAction::Continue,
                }
            }
            _ => EditorAction::Continue,
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }
    fn move_right(&mut self) {
        let len = self.lines[self.cursor_row].len();
        if self.cursor_col < len {
            self.cursor_col += 1;
        }
    }
    fn move_right_or_eol(&mut self) {
        // `a` in vim moves one past the cursor, clamped at EOL.
        let len = self.lines[self.cursor_row].len();
        self.cursor_col = (self.cursor_col + 1).min(len);
    }
    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }
    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }
}

pub trait MessageSender: Send + Sync {
    fn send_dm(&self, root: &Path, agent_id: &str, body: &str) -> Result<()>;
    fn broadcast(&self, root: &Path, channel_id: &str, body: &str) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CliMessageSender;

impl MessageSender for CliMessageSender {
    fn send_dm(&self, root: &Path, agent_id: &str, body: &str) -> Result<()> {
        let status = Command::new("teamctl")
            .arg("--root")
            .arg(root)
            .args(["send", agent_id, body])
            .status()
            .with_context(|| format!("invoke teamctl send {agent_id}"))?;
        if !status.success() {
            anyhow::bail!("teamctl send {agent_id} exited {status}");
        }
        Ok(())
    }

    fn broadcast(&self, root: &Path, channel_id: &str, body: &str) -> Result<()> {
        // `teamctl broadcast` takes a `#<name>` argument scoped to
        // the project's compose root. We pass the short name (after
        // the last `:`); the CLI resolves to the project's channel.
        let short = channel_id
            .rsplit_once(':')
            .map(|(_, n)| n)
            .unwrap_or(channel_id);
        let target = format!("#{short}");
        let status = Command::new("teamctl")
            .arg("--root")
            .arg(root)
            .args(["broadcast", &target, body])
            .status()
            .with_context(|| format!("invoke teamctl broadcast {target}"))?;
        if !status.success() {
            anyhow::bail!("teamctl broadcast {target} exited {status}");
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockMessageSender {
        pub dm_calls: Mutex<Vec<(String, String)>>,
        pub broadcast_calls: Mutex<Vec<(String, String)>>,
        /// When set, the next call returns an error of this text.
        /// Reset after firing so subsequent calls succeed (the
        /// modal's error-then-success path is a real flow).
        pub fail_next: Mutex<Option<String>>,
    }

    impl MessageSender for MockMessageSender {
        fn send_dm(&self, _root: &Path, agent_id: &str, body: &str) -> Result<()> {
            if let Some(err) = self.fail_next.lock().unwrap().take() {
                anyhow::bail!(err);
            }
            self.dm_calls
                .lock()
                .unwrap()
                .push((agent_id.into(), body.into()));
            Ok(())
        }
        fn broadcast(&self, _root: &Path, channel_id: &str, body: &str) -> Result<()> {
            if let Some(err) = self.fail_next.lock().unwrap().take() {
                anyhow::bail!(err);
            }
            self.broadcast_calls
                .lock()
                .unwrap()
                .push((channel_id.into(), body.into()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn k_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn dm_target_title_renders_as_arrow_agent() {
        let t = ComposeTarget::Dm {
            agent_id: "writing:dev1".into(),
            project_id: "writing".into(),
        };
        assert_eq!(t.title(), "→ writing:dev1");
    }

    #[test]
    fn broadcast_target_title_strips_project_prefix() {
        let t = ComposeTarget::Broadcast {
            channel_id: "writing:editorial".into(),
            project_id: "writing".into(),
        };
        assert_eq!(t.title(), "→ #editorial");
    }

    #[test]
    fn editor_starts_in_insert_mode() {
        let e = Editor::default();
        assert_eq!(e.mode, VimMode::Insert);
        assert!(e.is_empty());
    }

    #[test]
    fn typing_chars_appends_to_line() {
        let mut e = Editor::default();
        for c in "hello".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        assert_eq!(e.lines, vec!["hello"]);
        assert_eq!(e.cursor_col, 5);
        assert_eq!(e.body(), "hello");
    }

    #[test]
    fn enter_splits_line() {
        let mut e = Editor::default();
        for c in "hi".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        e.apply_key(k(KeyCode::Enter));
        for c in "yo".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        assert_eq!(e.lines, vec!["hi", "yo"]);
        assert_eq!(e.body(), "hi\nyo");
    }

    #[test]
    fn backspace_at_line_start_joins_with_previous() {
        let mut e = Editor::default();
        for c in "ab".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        e.apply_key(k(KeyCode::Enter));
        for c in "cd".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        // Cursor at start of line 2 → Backspace joins.
        e.cursor_col = 0;
        e.apply_key(k(KeyCode::Backspace));
        assert_eq!(e.lines, vec!["abcd"]);
        assert_eq!(e.cursor_row, 0);
        assert_eq!(e.cursor_col, 2);
    }

    #[test]
    fn esc_from_insert_drops_to_normal() {
        let mut e = Editor::default();
        e.apply_key(k(KeyCode::Esc));
        assert_eq!(e.mode, VimMode::Normal);
        assert!(e.esc_armed);
    }

    #[test]
    fn second_esc_cancels_from_any_mode() {
        let mut e = Editor::default();
        // From Insert: first Esc → Normal+armed; second Esc → Cancel.
        e.apply_key(k(KeyCode::Esc));
        assert_eq!(e.apply_key(k(KeyCode::Esc)), EditorAction::Cancel);

        // From Normal: first Esc arms; second Esc cancels.
        let mut e = Editor {
            mode: VimMode::Normal,
            ..Editor::default()
        };
        assert_eq!(e.apply_key(k(KeyCode::Esc)), EditorAction::Continue);
        assert_eq!(e.apply_key(k(KeyCode::Esc)), EditorAction::Cancel);
    }

    #[test]
    fn ctrl_enter_sends_from_any_mode() {
        let mut e = Editor::default();
        for c in "hi".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        assert_eq!(e.apply_key(k_ctrl(KeyCode::Enter)), EditorAction::Send);
    }

    #[test]
    fn ex_wq_sends() {
        let mut e = Editor::default();
        for c in "hi".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        // Esc → Normal, then `:wq` → Send.
        e.apply_key(k(KeyCode::Esc));
        e.apply_key(k(KeyCode::Char(':')));
        for c in "wq".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        assert_eq!(e.apply_key(k(KeyCode::Enter)), EditorAction::Send);
    }

    #[test]
    fn ex_q_cancels() {
        let mut e = Editor::default();
        e.apply_key(k(KeyCode::Esc));
        e.apply_key(k(KeyCode::Char(':')));
        e.apply_key(k(KeyCode::Char('q')));
        assert_eq!(e.apply_key(k(KeyCode::Enter)), EditorAction::Cancel);
    }

    #[test]
    fn normal_i_re_enters_insert() {
        let mut e = Editor::default();
        e.apply_key(k(KeyCode::Esc));
        // Non-Esc key clears the arm.
        e.apply_key(k(KeyCode::Char('i')));
        assert_eq!(e.mode, VimMode::Insert);
        assert!(!e.esc_armed);
    }

    #[test]
    fn hjkl_navigate_in_normal_mode() {
        let mut e = Editor::default();
        for c in "abc".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        e.apply_key(k(KeyCode::Esc));
        e.apply_key(k(KeyCode::Char('0')));
        assert_eq!(e.cursor_col, 0);
        e.apply_key(k(KeyCode::Char('l')));
        e.apply_key(k(KeyCode::Char('l')));
        assert_eq!(e.cursor_col, 2);
        e.apply_key(k(KeyCode::Char('h')));
        assert_eq!(e.cursor_col, 1);
    }

    #[test]
    fn body_strips_trailing_blank_lines() {
        let mut e = Editor::default();
        for c in "x".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        e.apply_key(k(KeyCode::Enter));
        e.apply_key(k(KeyCode::Enter));
        // body is `x\n\n` — strip both trailing newlines.
        assert_eq!(e.body(), "x");
    }
}
