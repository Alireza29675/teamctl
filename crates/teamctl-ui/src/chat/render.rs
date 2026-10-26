//! Chat widgets and message-identity anchored wrapping.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use team_core::chat::{ChatMessage, DeliveryStatus, Direction};

use super::{Focus, Selection, ThreadState};
use crate::app::App;
use crate::compose::VimMode;

pub struct ChatView<'a> {
    pub app: &'a App,
}

/// Narrow terminals retain a compact thread strip above the conversation.
pub fn geometry(area: Rect) -> [Rect; 3] {
    let (sidebar, main) = if area.width >= 72 {
        let columns = Layout::default()
            .direction(LayoutDirection::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(1)])
            .split(area);
        (columns[0], columns[1])
    } else {
        let rows = Layout::default()
            .direction(LayoutDirection::Vertical)
            .constraints([
                Constraint::Length(4.min(area.height / 3)),
                Constraint::Min(1),
            ])
            .split(area);
        (rows[0], rows[1])
    };
    let composer_height = 7.min(main.height / 2).max(u16::from(main.height > 0));
    let rows = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(composer_height)])
        .split(main);
    [sidebar, rows[0], rows[1]]
}

impl Widget for ChatView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [sidebar, history, composer] = geometry(area);
        render_conversations(self.app, sidebar, buf);
        render_history(self.app, history, buf);
        render_composer(self.app, composer, buf);
    }
}

fn block(app: &App, title: String, focus: Focus) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.chat.focus == focus {
            app.capabilities.accent()
        } else {
            app.capabilities.muted()
        }))
}

fn render_conversations(app: &App, area: Rect, buf: &mut Buffer) {
    let block = block(app, "CHAT · conversations".into(), Focus::Conversations);
    let inner = block.inner(area);
    block.render(area, buf);
    let agents: Vec<String> = app
        .team
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect();
    let entries = app.chat.entries(&agents);
    let mut lines = Vec::new();
    let mut selected_line = 0;
    for entry in entries {
        let selected = app.chat.selection.as_ref() == Some(&entry);
        let style = if selected {
            Style::default()
                .fg(app.capabilities.accent())
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        match entry {
            Selection::Conversation(id) => {
                let Some(conversation) = app
                    .chat
                    .conversations
                    .iter()
                    .find(|conversation| conversation.id == id)
                else {
                    continue;
                };
                let unread = app
                    .chat
                    .threads
                    .get(&id)
                    .map(|thread| thread.unread)
                    .unwrap_or(0);
                let suffix = if unread > 0 {
                    format!(" +{unread}")
                } else {
                    String::new()
                };
                lines.push(Line::styled(
                    format!(
                        "{} · {}{}",
                        clean(crate::data::agent_label(&app.team, &conversation.agent_id)),
                        clean(&conversation.adapter_id),
                        suffix
                    ),
                    style,
                ));
                lines.push(Line::styled(
                    format!(
                        "  {}",
                        compact_peer(
                            &conversation.peer_id,
                            inner.width.saturating_sub(2) as usize
                        )
                    ),
                    style,
                ));
            }
            Selection::NewAgent(id) => lines.push(Line::styled(
                format!(
                    "+ local: {}",
                    clean(crate::data::agent_label(&app.team, &id))
                ),
                style,
            )),
        }
        if selected {
            selected_line = lines.len().saturating_sub(1);
        }
    }
    let top = selected_line.saturating_sub(inner.height.saturating_sub(1) as usize);
    if lines.is_empty() {
        lines.push(Line::raw("No conversations yet"));
    }
    Paragraph::new(lines)
        .scroll((top.min(u16::MAX as usize) as u16, 0))
        .render(inner, buf);
}

fn compact_peer(peer: &str, width: usize) -> String {
    let peer = clean(peer);
    if peer.chars().count() <= width {
        return peer;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let tail = width.saturating_sub(1) / 2;
    let head = width.saturating_sub(1) - tail;
    format!(
        "{}…{}",
        peer.chars().take(head).collect::<String>(),
        peer.chars()
            .skip(peer.chars().count().saturating_sub(tail))
            .collect::<String>()
    )
}

fn render_history(app: &App, area: Rect, buf: &mut Buffer) {
    let title = app
        .chat
        .conversation()
        .map(|conversation| {
            format!(
                "{} · {} · {}",
                clean(crate::data::agent_label(&app.team, &conversation.agent_id)),
                clean(&conversation.adapter_id),
                clean(&conversation.peer_id)
            )
        })
        .unwrap_or_else(|| "Conversation · Enter opens a local chat".into());
    let block = block(app, title, Focus::History);
    let inner = block.inner(area);
    block.render(area, buf);
    let Some(thread) = app.chat.thread() else {
        Paragraph::new(if app.chat.open_pending {
            "Opening local chat…"
        } else {
            "Select a conversation or + local agent."
        })
        .style(Style::default().fg(app.capabilities.muted()))
        .render(inner, buf);
        return;
    };
    let reserved = u16::from(inner.height > 0);
    let height = inner.height.saturating_sub(reserved);
    let body = Rect { height, ..inner };
    if body.height == 0 || body.width == 0 {
        return;
    }
    let rows = cached_rows(thread, body.width.max(1));
    let top = top_line(thread, &rows, body.height as usize);
    if rows.is_empty() {
        Paragraph::new(if thread.refreshing {
            "Loading history…"
        } else {
            "No messages yet. Replies remain available after closing this client."
        })
        .style(Style::default().fg(app.capabilities.muted()))
        .render(body, buf);
    } else {
        Paragraph::new(
            rows.iter()
                .skip(top)
                .take(body.height as usize)
                .map(|row| row.line.clone())
                .collect::<Vec<_>>(),
        )
        .render(body, buf);
    }
    if reserved > 0 && inner.height > 0 {
        let hint = if let Some(error) = &thread.error {
            format!("History error: {error}")
        } else if thread.unread > 0 {
            format!("{} new · End follows replies", thread.unread)
        } else if thread.paging {
            "Loading older messages…".into()
        } else if thread.has_older {
            "PageUp loads older messages · End follows replies".into()
        } else {
            "End follows replies".into()
        };
        Paragraph::new(clean(&hint))
            .style(Style::default().fg(app.capabilities.muted()))
            .render(
                Rect {
                    y: inner.y + inner.height - 1,
                    height: 1,
                    ..inner
                },
                buf,
            );
    }
}

fn render_composer(app: &App, area: Rect, buf: &mut Buffer) {
    let block = block(app, "MESSAGE · local/operator".into(), Focus::Composer);
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }
    let read_only = app.chat.conversation().and_then(|conversation| {
        if conversation.adapter_id != "local" || conversation.peer_id != "operator" {
            Some("Remote history · open + local to message this agent")
        } else if !app
            .team
            .agents
            .iter()
            .any(|agent| agent.id == conversation.agent_id)
        {
            Some("Agent removed · history remains readable")
        } else {
            None
        }
    });
    if let Some(reason) = read_only {
        Paragraph::new(reason)
            .style(Style::default().fg(app.capabilities.muted()))
            .render(inner, buf);
        return;
    }
    let Some(thread) = app.chat.thread() else {
        if let Some(error) = app.chat.error.as_ref().or(app.chat.list_error.as_ref()) {
            Paragraph::new(clean(error)).render(inner, buf);
        }
        return;
    };
    let editor_area = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let lines: Vec<Line<'static>> = thread
        .editor
        .lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if index == thread.editor.cursor_row && app.chat.focus == Focus::Composer {
                let (head, tail) = text.split_at(thread.editor.cursor_byte_col());
                Line::from(vec![
                    Span::raw(clean(head)),
                    Span::styled("▏", Style::default().fg(app.capabilities.accent())),
                    Span::raw(clean(tail)),
                ])
            } else {
                Line::raw(clean(text))
            }
        })
        .collect();
    Paragraph::new(lines)
        .scroll(
            thread
                .editor
                .viewport(editor_area.width, editor_area.height),
        )
        .render(editor_area, buf);
    let hint = if let Some(submission) = &thread.submission {
        if submission.pending {
            "Sending… draft stays until accepted".into()
        } else {
            format!(
                "Send failed: {} · send again retries",
                submission.error.as_deref().unwrap_or("unknown error")
            )
        }
    } else if let Some(error) = app.chat.error.as_ref().or(app.chat.list_error.as_ref()) {
        format!("Chat error: {error}")
    } else if thread.editor.mode == VimMode::Normal {
        "Enter sends · Esc focuses conversations".into()
    } else {
        "Enter newline · Esc Enter / Alt+Enter send".into()
    };
    Paragraph::new(clean(&hint))
        .style(Style::default().fg(app.capabilities.muted()))
        .render(
            Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            },
            buf,
        );
}

#[derive(Debug)]
pub(crate) struct MessageRow {
    pub id: i64,
    pub offset: usize,
    pub line: Line<'static>,
}

#[derive(Debug, Default)]
pub(super) struct TranscriptCache {
    width: u16,
    rows: Vec<MessageRow>,
}

pub(crate) fn cached_rows(thread: &ThreadState, width: u16) -> std::cell::Ref<'_, Vec<MessageRow>> {
    if thread.render_cache.borrow().width != width {
        *thread.render_cache.borrow_mut() = TranscriptCache {
            width,
            rows: message_rows(&thread.messages, width),
        };
    }
    std::cell::Ref::map(thread.render_cache.borrow(), |cache| &cache.rows)
}

pub(crate) fn message_rows(messages: &[ChatMessage], width: u16) -> Vec<MessageRow> {
    let mut rows = Vec::new();
    for message in messages {
        let author = match message.direction {
            Direction::Human => "Human",
            Direction::Agent => "Agent",
        };
        let status = match message.status {
            DeliveryStatus::Queued => "queued",
            DeliveryStatus::Sent => "sent",
            DeliveryStatus::Failed => "failed",
            DeliveryStatus::Received => "received",
        };
        let time = chrono::DateTime::from_timestamp(message.created_at as i64, 0)
            .map(|time| time.format("%H:%M").to_string())
            .unwrap_or_else(|| "--:--".into());
        let mut lines = vec![format!("{time}  {author} · {status}")];
        if message.kind != "text" {
            lines.push(format!("[{}]", clean(&message.kind)));
        }
        lines.extend(message.text.lines().map(clean));
        if let Some(error) = &message.error {
            lines.push(format!("Error: {}", clean(error)));
        }
        lines.push(String::new());
        let mut offset = 0;
        for line in lines {
            for wrapped in wrap_line(&line, width.max(1) as usize) {
                let style = if offset == 0 {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                rows.push(MessageRow {
                    id: message.id,
                    offset,
                    line: Line::styled(wrapped, style),
                });
                offset += 1;
            }
        }
    }
    rows
}

pub(crate) fn top_line(thread: &ThreadState, rows: &[MessageRow], height: usize) -> usize {
    thread
        .anchor
        .and_then(|(id, offset)| {
            rows.iter()
                .position(|row| row.id == id && row.offset >= offset)
                .or_else(|| rows.iter().position(|row| row.id >= id))
        })
        .unwrap_or_else(|| rows.len().saturating_sub(height))
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut columns = 0;
    for character in text.chars() {
        let mut encoded = [0; 4];
        let encoded_character: &str = character.encode_utf8(&mut encoded);
        let cell_width = Span::raw(encoded_character).width();
        if columns + cell_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            columns = 0;
        }
        current.push(character);
        columns += cell_width;
    }
    lines.push(current);
    lines
}
