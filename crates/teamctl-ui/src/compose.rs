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

use anyhow::Result;
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
    /// Header text for the compose-modal title bar. DM targets render
    /// the agent's `display_name` when set (T-160 fallback) so the
    /// operator sees the same human label that surfaces in the roster
    /// and mailbox; broadcast targets render `#<channel-name>`
    /// unchanged. Pass `&app.team` so the agent lookup goes through
    /// the existing TeamSnapshot rather than carving a second context
    /// path.
    pub fn title(&self, team: &crate::data::TeamSnapshot) -> String {
        match self {
            ComposeTarget::Dm { agent_id, .. } => {
                let label = crate::data::agent_label(team, agent_id);
                format!("→ {label}")
            }
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
    /// UTF-8 byte offset, always positioned at a character boundary.
    pub cursor_col: usize,
    pub mode: VimMode,
    pub ex_buffer: String,
    /// Tracks whether the previous keypress was `Esc`. Two Escs in
    /// a row from any mode cancel the surrounding modal — same
    /// shape SPEC §4 specifies for "close all modals."
    pub esc_armed: bool,
    /// Single-register yank buffer for `yy` / `dd` → `p`. Only
    /// holds full lines; word-level yanks aren't in PR-UI-7 scope.
    /// Empty `Vec` means "nothing to paste."
    pub yank: Vec<String>,
    /// Tracks whether the previous Normal-mode key was `d` or `y`.
    /// `dd` deletes the line, `yy` yanks it. The pending-op state
    /// clears on any non-matching key so `dx` doesn't leave the
    /// editor in a half-operation.
    pub pending_op: Option<char>,
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
            yank: Vec::new(),
            pending_op: None,
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
    /// Clamp externally supplied cursor offsets before slicing text.
    pub fn cursor_byte_col(&self) -> usize {
        self.lines
            .get(self.cursor_row)
            .map(|line| boundary_at_or_before(line, self.cursor_col))
            .unwrap_or(0)
    }

    /// Keep the active row and caret visible without wrapping the draft.
    pub fn viewport(&self, width: u16, height: u16) -> (u16, u16) {
        let top = self
            .cursor_row
            .saturating_sub(height.saturating_sub(1) as usize);
        let col = self.cursor_byte_col();
        let display_col = self
            .lines
            .get(self.cursor_row)
            .map(|line| ratatui::text::Span::raw(&line[..col]).width())
            .unwrap_or(0);
        let left = display_col.saturating_sub(width.saturating_sub(1) as usize);
        (
            top.min(u16::MAX as usize) as u16,
            left.min(u16::MAX as usize) as u16,
        )
    }

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

        // Send chord — Alt+Enter is the terminal-universal send
        // (xterm / Terminal.app / tmux deliver it as Enter+ALT in
        // their default modes). Ctrl+Enter is kept for terminals
        // running the kitty keyboard protocol or modifyOtherKeys,
        // where it does decode distinctly. Either chord fires send
        // from any editor mode.
        if k.code == KeyCode::Enter
            && (k.modifiers.contains(KeyModifiers::ALT)
                || k.modifiers.contains(KeyModifiers::CONTROL))
        {
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
            KeyCode::Char(c) if k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT => {
                let line = &mut self.lines[self.cursor_row];
                let col = boundary_at_or_before(line, self.cursor_col);
                line.insert(col, c);
                self.cursor_col = col + c.len_utf8();
            }
            KeyCode::Enter => {
                let line = &mut self.lines[self.cursor_row];
                let col = boundary_at_or_before(line, self.cursor_col);
                let tail = line.split_off(col);
                self.cursor_row += 1;
                self.lines.insert(self.cursor_row, tail);
                self.cursor_col = 0;
            }
            KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    let line = &mut self.lines[self.cursor_row];
                    let col = boundary_at_or_before(line, self.cursor_col);
                    let previous = previous_boundary(line, col);
                    if previous < col {
                        line.remove(previous);
                    }
                    self.cursor_col = previous;
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
        // Pending-op (dd/yy) machine: when the previous press was
        // `d` or `y`, the next key either completes the op
        // (`d`/`y`) or aborts it. Any non-d/y/p key clears the
        // pending-op so we don't leave the editor stuck.
        if let Some(op) = self.pending_op {
            self.pending_op = None;
            match (op, k.code) {
                ('d', KeyCode::Char('d')) => {
                    self.delete_line();
                    return EditorAction::Continue;
                }
                ('y', KeyCode::Char('y')) => {
                    self.yank_line();
                    return EditorAction::Continue;
                }
                _ => {} // fall through and re-dispatch the key
            }
        }
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
            // PR-UI-7 word motions — `w` next word, `b` prev
            // word, `e` end of word. ASCII-word semantics
            // (alphanumeric + `_`); good enough for prose +
            // single-line code we'd compose into mailbox messages.
            KeyCode::Char('w') => self.move_word_forward(),
            KeyCode::Char('b') => self.move_word_back(),
            KeyCode::Char('e') => self.move_word_end(),
            // PR-UI-7 line ops — first press arms the op, second
            // press completes. `p` pastes the yank register
            // below the current line.
            KeyCode::Char('d') => self.pending_op = Some('d'),
            KeyCode::Char('y') => self.pending_op = Some('y'),
            KeyCode::Char('p') => self.paste_below(),
            // T-313: plain Enter in Normal mode is the terminal-
            // universal submit (Esc → Enter). It carries no modifier
            // and no kitty/modifyOtherKeys dependency, so it works on
            // the default-mode terminals where Alt/Ctrl+Enter is eaten.
            // Normal mode never inserts text, so this doesn't fight
            // newline entry (that stays Insert-mode Enter).
            KeyCode::Enter => return EditorAction::Send,
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
        self.cursor_col = previous_boundary(&self.lines[self.cursor_row], self.cursor_byte_col());
    }
    fn move_right(&mut self) {
        self.cursor_col = next_boundary(&self.lines[self.cursor_row], self.cursor_byte_col());
    }
    fn move_right_or_eol(&mut self) {
        // `a` in vim moves one past the cursor, clamped at EOL.
        self.move_right();
    }
    fn move_word_forward(&mut self) {
        let line = &self.lines[self.cursor_row];
        let col = self.cursor_byte_col();
        let mut chars = line[col..].char_indices().peekable();
        while chars.peek().is_some_and(|(_, c)| is_word_char(*c)) {
            chars.next();
        }
        while chars.peek().is_some_and(|(_, c)| !is_word_char(*c)) {
            chars.next();
        }
        let next = chars.peek().map(|(i, _)| col + i).unwrap_or(line.len());
        if next == col && self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        } else {
            self.cursor_col = next;
        }
    }

    fn move_word_back(&mut self) {
        let col = self.cursor_byte_col();
        if col == 0 {
            if self.cursor_row > 0 {
                self.cursor_row -= 1;
                self.cursor_col = self.lines[self.cursor_row].len();
            }
            return;
        }
        let line = &self.lines[self.cursor_row];
        let mut chars = line[..col].char_indices().rev().peekable();
        while chars.peek().is_some_and(|(_, c)| !is_word_char(*c)) {
            chars.next();
        }
        let mut next = 0;
        while chars.peek().is_some_and(|(_, c)| is_word_char(*c)) {
            next = chars.next().expect("peeked character").0;
        }
        self.cursor_col = next;
    }

    fn move_word_end(&mut self) {
        let line = &self.lines[self.cursor_row];
        let col = self.cursor_byte_col();
        let mut chars = line[col..].char_indices().peekable();
        // At the end of a word, find the next word before its end.
        if chars.peek().is_some_and(|(_, c)| is_word_char(*c)) {
            let mut probe = chars.clone();
            probe.next();
            if probe.peek().is_none_or(|(_, c)| !is_word_char(*c)) {
                chars.next();
            }
        }
        while chars.peek().is_some_and(|(_, c)| !is_word_char(*c)) {
            chars.next();
        }
        let mut next = col;
        while chars.peek().is_some_and(|(_, c)| is_word_char(*c)) {
            next = col + chars.next().expect("peeked character").0;
        }
        self.cursor_col = next;
    }

    fn delete_line(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        let removed = self.lines.remove(self.cursor_row);
        self.yank = vec![removed];
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len() - 1;
        }
        self.cursor_col = self.cursor_byte_col();
    }
    fn yank_line(&mut self) {
        if let Some(line) = self.lines.get(self.cursor_row) {
            self.yank = vec![line.clone()];
        }
    }
    fn paste_below(&mut self) {
        if self.yank.is_empty() {
            return;
        }
        let yanked = self.yank.clone();
        for (offset, line) in yanked.into_iter().enumerate() {
            self.lines.insert(self.cursor_row + 1 + offset, line);
        }
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_byte_col();
        }
    }
    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_byte_col();
        }
    }
}

fn boundary_at_or_before(line: &str, col: usize) -> usize {
    let mut col = col.min(line.len());
    while !line.is_char_boundary(col) {
        col -= 1;
    }
    col
}

fn previous_boundary(line: &str, col: usize) -> usize {
    line[..col]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_boundary(line: &str, col: usize) -> usize {
    col + line[col..].chars().next().map(char::len_utf8).unwrap_or(0)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub trait MessageSender: Send + Sync {
    fn send_dm(&self, root: &Path, agent_id: &str, body: &str) -> Result<()>;
    fn broadcast(&self, root: &Path, channel_id: &str, body: &str) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CliMessageSender;

fn message_command(root: &Path, verb: &str, target: &str, body: &str) -> Command {
    let mut command = Command::new("teamctl");
    command
        .arg("--root")
        .arg(root)
        .args([verb, "--", target, body]);
    command
}

impl MessageSender for CliMessageSender {
    fn send_dm(&self, root: &Path, agent_id: &str, body: &str) -> Result<()> {
        crate::command::run(
            &mut message_command(root, "send", agent_id, body),
            &format!("invoke teamctl send {agent_id}"),
        )
    }

    fn broadcast(&self, root: &Path, channel_id: &str, body: &str) -> Result<()> {
        // Retain the project qualifier: different projects may declare
        // identically named channels.
        crate::command::run(
            &mut message_command(root, "broadcast", channel_id, body),
            &format!("invoke teamctl broadcast {channel_id}"),
        )
    }
}

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

    mod regressions;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn k_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn empty_team() -> crate::data::TeamSnapshot {
        crate::data::TeamSnapshot::empty(std::path::PathBuf::from("/tmp"))
    }

    #[test]
    fn dm_target_title_renders_as_arrow_agent() {
        let team = empty_team();
        let t = ComposeTarget::Dm {
            agent_id: "writing:dev1".into(),
            project_id: "writing".into(),
        };
        // Empty team → no display_name override; falls through to the
        // canonical id.
        assert_eq!(t.title(&team), "→ writing:dev1");
    }

    #[test]
    fn dm_target_title_uses_display_name_when_set() {
        // T-160: DM modal title swaps in `display_name` when the
        // target agent has one in the team snapshot.
        use crate::data::{AgentInfo, TeamSnapshot};
        use team_core::supervisor::AgentState;
        let agent = AgentInfo {
            id: "writing:dev1".into(),
            agent: "dev1".into(),
            project: "writing".into(),
            tmux_session: "a-writing-dev1".into(),
            state: AgentState::Unknown,
            unread_mail: 0,
            pending_approvals: 0,
            is_manager: false,
            display_name: Some("Dev 1 (Drafter)".into()),
            rate_limit_resets_at: None,
            last_activity_at: None,
            reports_to: None,
        };
        let team = TeamSnapshot {
            root: std::path::PathBuf::from("/tmp"),
            team_name: "t".into(),
            agents: vec![agent],
            channels: vec![],
        };
        let t = ComposeTarget::Dm {
            agent_id: "writing:dev1".into(),
            project_id: "writing".into(),
        };
        assert_eq!(t.title(&team), "→ Dev 1 (Drafter)");
    }

    #[test]
    fn broadcast_target_title_strips_project_prefix() {
        let team = empty_team();
        let t = ComposeTarget::Broadcast {
            channel_id: "writing:editorial".into(),
            project_id: "writing".into(),
        };
        assert_eq!(t.title(&team), "→ #editorial");
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
    fn normal_mode_enter_sends() {
        // T-313: plain Enter in Normal mode is the terminal-universal
        // submit (Esc → Enter). No modifier, no kitty/modifyOtherKeys
        // dependency — works where Alt/Ctrl+Enter is eaten.
        let mut e = Editor::default();
        for c in "hi".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        e.apply_key(k(KeyCode::Esc)); // Insert → Normal
        assert_eq!(e.apply_key(k(KeyCode::Enter)), EditorAction::Send);
    }

    #[test]
    fn insert_mode_enter_still_inserts_newline_not_send() {
        // T-313 regression pin: the universal submit is Normal-mode
        // only. Plain Enter while typing must still add a line, or
        // multi-line compose breaks.
        let mut e = Editor::default();
        for c in "ab".chars() {
            e.apply_key(k(KeyCode::Char(c)));
        }
        assert_eq!(e.apply_key(k(KeyCode::Enter)), EditorAction::Continue);
        assert_eq!(e.lines.len(), 2, "Insert Enter must split the line");
    }

    #[test]
    fn alt_enter_still_sends_for_kitty_protocol_terminals() {
        // T-313: don't regress the advanced-terminal fast path.
        let mut e = Editor::default();
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(e.apply_key(alt_enter), EditorAction::Send);
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
