//! A single input/render loop keeps incoming replies from overwriting a draft.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Result};
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use rusqlite::Connection;
use team_core::chat::{Conversation, Repository};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Default)]
struct Input {
    text: String,
    cursor: usize,
}

impl Input {
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8)
    }

    fn insert(&mut self, c: char) -> bool {
        if c.is_control() || self.text.len() + c.len_utf8() > team_core::chat::MAX_TEXT_BYTES {
            return false;
        }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        true
    }

    fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let previous = self.previous();
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
        true
    }

    fn delete(&mut self) -> bool {
        if self.cursor == self.text.len() {
            return false;
        }
        self.text.drain(self.cursor..self.next());
        true
    }
}

struct Attempt {
    body: String,
    request_id: String,
}

#[derive(Default)]
struct Session {
    input: Input,
    attempt: Option<Attempt>,
    send_error: Option<String>,
}

impl Session {
    fn request_id(&mut self) -> &str {
        let unchanged = self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.body == self.input.text);
        if !unchanged {
            self.attempt = Some(Attempt {
                body: self.input.text.clone(),
                request_id: super::request_id(),
            });
        }
        &self
            .attempt
            .as_ref()
            .expect("submission attempt exists")
            .request_id
    }

    fn edited(&mut self, changed: bool) {
        if changed {
            // An uncertain send may already have committed. Only an unchanged
            // body can safely reuse that submission's identity.
            self.attempt = None;
            self.send_error = None;
        }
    }

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let changed = match code {
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                *self = Self::default();
                return;
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => self.input.insert(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => {
                self.input.cursor = self.input.previous();
                false
            }
            KeyCode::Right => {
                self.input.cursor = self.input.next();
                false
            }
            KeyCode::Home => {
                self.input.cursor = 0;
                false
            }
            KeyCode::End => {
                self.input.cursor = self.input.text.len();
                false
            }
            _ => false,
        };
        self.edited(changed);
    }

    fn paste(&mut self, text: &str) {
        let mut changed = false;
        for c in text.chars() {
            // Pasted line endings never become an implicit Enter/send event.
            let c = if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            };
            changed |= self.input.insert(c);
        }
        self.edited(changed);
    }
}

pub(super) fn run(conn: &Connection, conversation: &Conversation) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("interactive chat needs a terminal; use `chat send`, `chat history --json` or `chat follow --json`");
    }
    if conversation.adapter_id != "local" || conversation.peer_id != "operator" {
        bail!("external conversations are read-only here; send from their messenger");
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1)",
        [&conversation.agent_id],
        |row| row.get(0),
    )?;
    if !exists {
        bail!(
            "agent {} was removed; use `chat history {}` to read its transcript",
            conversation.agent_id,
            conversation.id
        );
    }
    conn.busy_timeout(Duration::from_millis(200))?;
    let repo = Repository::new(conn);
    let mut transcript = super::transcript::Transcript::new(25, 100);
    println!(
        "Chat with {} · Enter sends · Ctrl+C/D closes",
        super::safe_text(&conversation.agent_id)
    );
    for row in transcript.poll(&repo, &conversation.id)? {
        super::print_message(&row, conversation);
    }
    terminal::enable_raw_mode()?;
    let _raw = RawMode;
    let mut stdout = io::stdout();
    execute!(stdout, EnableBracketedPaste)?;
    let mut session = Session::default();
    let mut read_error = None;
    loop {
        // Reads are bounded. Temporary contention leaves the prompt usable.
        match transcript.poll(&repo, &conversation.id) {
            Ok(rows) => {
                for row in rows {
                    print_line(&mut stdout, &super::message_text(&row, conversation))?;
                }
                read_error = None;
            }
            Err(error) => {
                let error = error.to_string();
                if read_error.as_ref() != Some(&error) {
                    print_line(&mut stdout, &format!("history unavailable: {error}"))?;
                }
                read_error = Some(error);
            }
        }
        render_prompt(&mut stdout, &session.input, session.send_error.is_some())?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Paste(text) => session.paste(&text),
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c' | 'd'))
                {
                    break;
                }
                if key.code == KeyCode::Enter && !session.input.text.is_empty() {
                    let id = session.request_id().to_owned();
                    match repo.send_local(&conversation.id, &session.input.text, &id) {
                        Ok(_) => {
                            // Do not advance the read cursor from a send: an
                            // agent reply may precede this row. The next read
                            // displays both rows once, in transcript order.
                            session = Session::default();
                        }
                        Err(error) => {
                            let error = error.to_string();
                            print_line(&mut stdout, &format!("send failed: {error} · Enter retries; editing starts a new request"))?;
                            session.send_error = Some(error);
                        }
                    }
                } else {
                    session.key(key.code, key.modifiers);
                }
            }
            _ => {}
        }
    }
    execute!(
        stdout,
        cursor::MoveToColumn(0),
        Clear(ClearType::CurrentLine)
    )?;
    writeln!(stdout, "\r")?;
    Ok(())
}

fn print_line(out: &mut impl Write, text: &str) -> Result<()> {
    execute!(out, cursor::MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    writeln!(out, "{}\r", super::safe_text(text).replace('\n', "\r\n"))?;
    Ok(())
}

fn clip(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

fn prompt(input: &Input, failed: bool, terminal_width: u16) -> (String, u16) {
    // Reserve the final column so printing never wraps underneath the prompt.
    let available = terminal_width.saturating_sub(1) as usize;
    let prefix = clip(if failed { "retry> " } else { "you> " }, available);
    let prefix_width = prefix.width();
    let body_width = available.saturating_sub(prefix_width);
    if body_width == 0 {
        return (prefix, prefix_width as u16);
    }
    let mut start = input.cursor;
    let mut used = 0;
    for (index, c) in input.text[..input.cursor].char_indices().rev() {
        used += c.width().unwrap_or(0);
        if used >= body_width {
            break;
        }
        start = index;
    }
    let column = prefix_width + input.text[start..input.cursor].width();
    (
        format!("{prefix}{}", clip(&input.text[start..], body_width)),
        column as u16,
    )
}

fn render_prompt(out: &mut impl Write, input: &Input, failed: bool) -> Result<()> {
    let (text, column) = prompt(input, failed, terminal::size()?.0);
    execute!(out, cursor::MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    write!(out, "{text}")?;
    execute!(out, cursor::MoveToColumn(column))?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_cursor_editing_preserves_scalar_boundaries() {
        let mut input = Input::default();
        for c in "سلام é 📎".chars() {
            input.insert(c);
        }
        input.backspace();
        assert_eq!(input.text, "سلام é ");
        input.cursor = input.previous();
        input.insert('界');
        assert!(input.text.is_char_boundary(input.cursor));
        input.backspace();
        assert_eq!(input.text, "سلام é ");
        input.cursor = 0;
        input.delete();
        assert_eq!(input.text, "لام é ");
    }

    #[test]
    fn unchanged_retry_keeps_identity_but_any_edit_invalidates_it() {
        let mut session = Session::default();
        session.paste("hello");
        let id = session.request_id().to_owned();
        session.send_error = Some("uncertain result".into());
        session.key(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(session.request_id(), id);
        assert!(session.send_error.is_some());
        session.key(KeyCode::Char('!'), KeyModifiers::NONE);
        session.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(session.input.text, "hello");
        assert_ne!(session.request_id(), id);
        assert!(session.send_error.is_none());
        session.key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(session.input.text.is_empty());
        assert!(session.attempt.is_none());
    }

    #[test]
    fn paste_and_shortcut_letters_are_literal_and_bounded() {
        let mut session = Session::default();
        session.paste("سلام\n界\t\u{1b}");
        session.key(KeyCode::Char('q'), KeyModifiers::NONE);
        session.key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(session.input.text, "سلام 界 qc");
        session.input = Input {
            text: "a".repeat(team_core::chat::MAX_TEXT_BYTES - 1),
            cursor: team_core::chat::MAX_TEXT_BYTES - 1,
        };
        assert!(!session.input.insert('界'));
        assert!(session.input.insert('b'));
        assert!(!session.input.insert('c'));
    }

    #[test]
    fn narrow_prompt_never_wraps_or_splits_unicode() {
        let text = "سلام e\u{301} 界 📎";
        for cursor in text
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
        {
            let input = Input {
                text: text.into(),
                cursor,
            };
            for width in 0..40 {
                for failed in [false, true] {
                    let (line, column) = prompt(&input, failed, width);
                    assert!(
                        line.width() <= width.saturating_sub(1) as usize,
                        "{width}: {line:?}"
                    );
                    assert!(column <= width.saturating_sub(1));
                }
            }
        }
    }

    #[test]
    fn maximum_draft_uses_a_small_prompt_viewport() {
        let text = "a".repeat(team_core::chat::MAX_TEXT_BYTES);
        let input = Input {
            cursor: text.len(),
            text,
        };
        let (line, column) = prompt(&input, false, 40);
        assert_eq!(line, format!("you> {}", "a".repeat(33)));
        assert_eq!(column, 38);
    }
}
