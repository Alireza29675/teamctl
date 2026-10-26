//! Chat-specific dispatch runs before dashboard shortcuts.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use super::{render, Focus, Selection};
use crate::app::App;
use crate::compose::{EditorAction, VimMode};
use crate::triptych::MainLayout;

impl App {
    pub fn enter_chat(&mut self) {
        if self.layout != MainLayout::Chat {
            self.chat_dashboard_layout = self.layout;
        }
        self.layout = MainLayout::Chat;
        self.pending_chord = None;
        self.chat.refresh();
        if let Some(agent_id) = self.selected_agent_id() {
            let existing = self
                .chat
                .conversations
                .iter()
                .find(|conversation| {
                    conversation.adapter_id == "local"
                        && conversation.peer_id == "operator"
                        && conversation.agent_id == agent_id
                })
                .map(|conversation| conversation.id.clone());
            if let Some(id) = existing {
                self.chat.select(Selection::Conversation(id));
            } else {
                self.chat.select(Selection::NewAgent(agent_id.clone()));
                self.chat.open_agent(agent_id);
            }
        }
        self.chat.focus = Focus::Composer;
    }

    pub fn leave_chat(&mut self) {
        self.layout = self.chat_dashboard_layout;
        self.pending_chord = None;
    }

    pub(crate) fn handle_chat_event(&mut self, event: Event) {
        let agents: Vec<String> = self
            .team
            .agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match key.code {
                    KeyCode::Char('d' | 'D') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.leave_chat()
                    }
                    KeyCode::Char('q' | 'Q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.enter_quit_confirm()
                    }
                    KeyCode::Tab => {
                        self.chat.focus = self
                            .chat
                            .focus
                            .next(key.modifiers.contains(KeyModifiers::SHIFT))
                    }
                    KeyCode::BackTab => self.chat.focus = self.chat.focus.next(true),
                    KeyCode::PageUp => self.chat_scroll(-10),
                    KeyCode::PageDown => self.chat_scroll(10),
                    KeyCode::End => self.chat_follow_tail(),
                    _ if self.chat.focus == Focus::Composer => {
                        // Remote and removed-agent conversations are readable. Only an
                        // explicit local/operator destination may accept a draft/send.
                        let writable = self.chat.conversation().is_some_and(|conversation| {
                            conversation.adapter_id == "local"
                                && conversation.peer_id == "operator"
                                && agents.contains(&conversation.agent_id)
                        });
                        if !writable {
                            if key.code == KeyCode::Esc {
                                self.chat.focus = Focus::Conversations;
                            }
                            return;
                        }
                        let action = if let Some(thread) = self.chat.thread_mut() {
                            let added = match key.code {
                                KeyCode::Char(character) => character.len_utf8(),
                                KeyCode::Enter => 1,
                                _ => 0,
                            };
                            if added > 0
                                && !(key.code == KeyCode::Enter && thread.editor.esc_armed)
                                && !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && thread.editor.lines.iter().map(String::len).sum::<usize>()
                                    + thread.editor.lines.len().saturating_sub(1)
                                    + added
                                    > team_core::chat::MAX_TEXT_BYTES
                            {
                                self.chat.error = Some(
                                    "Message is limited to 64 KiB; remove text before sending."
                                        .into(),
                                );
                                return;
                            }
                            if key.code == KeyCode::Esc && thread.editor.esc_armed {
                                thread.editor.esc_armed = false;
                                self.chat.focus = Focus::Conversations;
                                return;
                            }
                            // Esc→Enter is the send chord. Every other character stays
                            // literal insert text, including q/c/@/! after a single Esc.
                            if key.code != KeyCode::Esc
                                && !(key.code == KeyCode::Enter && thread.editor.esc_armed)
                            {
                                thread.editor.mode = VimMode::Insert;
                            }
                            thread.editor.apply_key(key)
                        } else {
                            EditorAction::Continue
                        };
                        if action == EditorAction::Send {
                            self.chat.send(&agents);
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') if self.chat.focus == Focus::Conversations => {
                        self.chat.navigate(&agents, true)
                    }
                    KeyCode::Down | KeyCode::Char('j')
                        if self.chat.focus == Focus::Conversations =>
                    {
                        self.chat.navigate(&agents, false)
                    }
                    KeyCode::Enter if self.chat.focus == Focus::Conversations => {
                        if let Some(Selection::NewAgent(agent_id)) = self.chat.selection.clone() {
                            self.chat.open_agent(agent_id);
                        } else {
                            self.chat.focus = Focus::Composer;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') if self.chat.focus == Focus::History => {
                        self.chat_scroll(-1)
                    }
                    KeyCode::Down | KeyCode::Char('j') if self.chat.focus == Focus::History => {
                        self.chat_scroll(1)
                    }
                    KeyCode::Home if self.chat.focus == Focus::History => {
                        if let Some(thread) = self.chat.thread_mut() {
                            thread.anchor = thread.messages.first().map(|message| (message.id, 0));
                        }
                        self.chat.page_older();
                    }
                    KeyCode::Char('q') => self.enter_quit_confirm(),
                    KeyCode::Char('c') | KeyCode::Esc => self.leave_chat(),
                    KeyCode::Char('?') => self.enter_help_overlay(),
                    _ => {}
                }
            }
            Event::Paste(text) if self.chat.focus == Focus::Composer => {
                let writable = self.chat.conversation().is_some_and(|conversation| {
                    conversation.adapter_id == "local"
                        && conversation.peer_id == "operator"
                        && agents.contains(&conversation.agent_id)
                });
                if writable {
                    if let Some(thread) = self.chat.thread_mut() {
                        thread.editor.mode = VimMode::Insert;
                        let mut bytes = thread.editor.lines.iter().map(String::len).sum::<usize>()
                            + thread.editor.lines.len().saturating_sub(1);
                        for character in text.chars().take(65536) {
                            let code = if character == '\n' {
                                KeyCode::Enter
                            } else if character == '\r' {
                                continue;
                            } else {
                                KeyCode::Char(character)
                            };
                            if bytes + character.len_utf8() > team_core::chat::MAX_TEXT_BYTES {
                                break;
                            }
                            thread.editor.apply_key(crossterm::event::KeyEvent::new(
                                code,
                                KeyModifiers::NONE,
                            ));
                            bytes += character.len_utf8();
                        }
                    }
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.chat_scroll(-3),
                MouseEventKind::ScrollDown => self.chat_scroll(3),
                _ => {}
            },
            _ => {}
        }
    }

    fn chat_follow_tail(&mut self) {
        if let Some(thread) = self.chat.thread_mut() {
            if !thread.reload_tail {
                thread.anchor = None;
                // Historical windows may have evicted newer rows. Re-entering
                // tail loads its newest page, then acknowledges unread rows.
                thread.messages.clear();
                *thread.render_cache.get_mut() = render::TranscriptCache::default();
                thread.newest_seen = None;
                thread.reload_tail = true;
                thread.refreshing = false;
                thread.new_pending = false;
                thread.paging = false;
            }
        }
        self.chat.refresh_selected();
    }

    fn chat_scroll(&mut self, amount: isize) {
        let (width, height) = self.chat_viewport;
        let mut page = false;
        let mut reload_tail = false;
        if let Some(thread) = self.chat.thread_mut() {
            let (next, max_top, anchor) = {
                let lines = render::cached_rows(thread, width.max(1));
                if lines.is_empty() {
                    return;
                }
                let top = render::top_line(thread, &lines, height as usize);
                let max_top = lines.len().saturating_sub(height as usize);
                let next = top.saturating_add_signed(amount).min(max_top);
                (
                    next,
                    max_top,
                    lines.get(next).map(|line| (line.id, line.offset)),
                )
            };
            if next == max_top && amount > 0 {
                if thread.tail_evicted() {
                    reload_tail = true;
                } else {
                    thread.anchor = None;
                    thread.unread = 0;
                }
            } else {
                thread.anchor = anchor;
            }
            page = next == 0 && amount < 0;
        }
        if reload_tail {
            self.chat_follow_tail();
        } else if page {
            self.chat.page_older();
        }
    }
}
