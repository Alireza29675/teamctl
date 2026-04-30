//! Triptych — the default Layout A. Three resizable panes (roster,
//! detail, mailbox) with an Approvals stripe reserved at the top
//! (rendered only when there's something to surface — empty in
//! PR-UI-2 still) and a focus ring on the active pane.
//!
//! PR-UI-2 wires the roster + detail panes to live data:
//! - Roster lists `app.team.agents` with single-cell state glyphs
//!   driven by `data::state_glyph`. Selection is highlighted with
//!   the focus accent.
//! - Detail shows the last-N lines of `app.detail_buffer` (the
//!   tmux capture-pane scrollback for the focused agent), or an
//!   empty-state hint when no agent is selected.
//! - Mailbox stays empty-state — wiring lands in PR-UI-3.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::app::App;
use crate::data::{state_glyph, AgentInfo};
use crate::mailbox::{render_row, MailboxTab};
use crate::theme::ColorMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Roster,
    Detail,
    Mailbox,
}

impl Pane {
    /// `Tab` cycles in roster → detail → mailbox → roster order.
    pub fn next(self) -> Self {
        match self {
            Pane::Roster => Pane::Detail,
            Pane::Detail => Pane::Mailbox,
            Pane::Mailbox => Pane::Roster,
        }
    }
}

pub fn draw(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    Triptych { app }.render(area, f.buffer_mut());
}

pub struct Triptych<'a> {
    pub app: &'a App,
}

impl Widget for Triptych<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // The approvals stripe takes one line at the top *only* when
        // there's a pending approval. PR-UI-2 still leaves the
        // stripe hidden — wiring lands in PR-UI-4.
        let stripe_visible = false;
        let body = if stripe_visible {
            let v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);
            v[1]
        } else {
            area
        };

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(28), // roster
                Constraint::Min(0),     // detail
                Constraint::Length(32), // mailbox
            ])
            .split(body);

        render_roster(buf, columns[0], self.app);
        render_detail(buf, columns[1], self.app);
        render_mailbox(buf, columns[2], self.app);
    }
}

fn render_roster(buf: &mut Buffer, area: Rect, app: &App) {
    let focused = app.focused_pane == Pane::Roster;
    let block = pane_block("ROSTER", focused, app);
    let inner = block.inner(area);
    block.render(area, buf);

    if app.team.agents.is_empty() {
        let empty = Paragraph::new("(no agents)")
            .style(Style::default().fg(app.capabilities.muted()))
            .alignment(Alignment::Center);
        empty.render(inner, buf);
        return;
    }

    let ascii = matches!(app.capabilities.color, ColorMode::Monochrome);
    let lines: Vec<Line<'_>> = app
        .team
        .agents
        .iter()
        .enumerate()
        .map(|(i, info)| roster_line(info, Some(i) == app.selected_agent, ascii, app))
        .collect();
    let para = Paragraph::new(lines).alignment(Alignment::Left);
    para.render(inner, buf);
}

fn roster_line<'a>(info: &'a AgentInfo, selected: bool, ascii: bool, app: &App) -> Line<'a> {
    let glyph = state_glyph(info, ascii);
    let display = format!(" {glyph}  {}", info.agent);
    let style = if selected {
        Style::default()
            .fg(app.capabilities.accent())
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    Line::styled(display, style)
}

fn render_detail(buf: &mut Buffer, area: Rect, app: &App) {
    let focused = app.focused_pane == Pane::Detail;
    let title = match app
        .selected_agent
        .and_then(|i| app.team.agents.get(i))
        .map(|a| a.id.as_str())
    {
        Some(id) => format!("DETAIL · {id}"),
        None => "DETAIL".to_string(),
    };
    let block = pane_block(&title, focused, app);
    let inner = block.inner(area);
    block.render(area, buf);

    if app.selected_agent.is_none() || app.team.agents.is_empty() {
        let muted = Style::default().fg(app.capabilities.muted());
        Paragraph::new("(select an agent on the left to follow its session)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    if app.detail_buffer.is_empty() {
        let muted = Style::default().fg(app.capabilities.muted());
        Paragraph::new("(no scrollback yet — agent may be starting up)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }

    // Tail the buffer to whatever fits; ratatui already clips lines
    // that overrun the rect, but pre-trimming saves a render-time
    // copy of thousands of lines we'd never see.
    let cap = inner.height as usize;
    let start = app.detail_buffer.len().saturating_sub(cap);
    let lines: Vec<Line<'_>> = app.detail_buffer[start..]
        .iter()
        .map(|s| Line::raw(s.clone()))
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

fn render_mailbox(buf: &mut Buffer, area: Rect, app: &App) {
    let focused = app.focused_pane == Pane::Mailbox;
    let block = pane_block("MAILBOX", focused, app);
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    // Reserve the top line for the tab indicator; everything below
    // is rows from the active tab's buffer.
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_mailbox_tabs(buf, layout[0], app);
    render_mailbox_body(buf, layout[1], app);
}

fn render_mailbox_tabs(buf: &mut Buffer, area: Rect, app: &App) {
    // `Inbox  Channel  Wire` — active tab gets the focus accent
    // (REVERSED so it reads as a highlight bar even in monochrome
    // terminals where colour alone wouldn't carry the signal).
    let active_style = Style::default()
        .fg(app.capabilities.accent())
        .add_modifier(Modifier::REVERSED);
    let muted = Style::default().fg(app.capabilities.muted());
    let mut spans: Vec<ratatui::text::Span<'_>> = Vec::with_capacity(7);
    for (i, tab) in MailboxTab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(ratatui::text::Span::styled("  ", muted));
        }
        let label = format!(" {} ", tab.label());
        let style = if app.mailbox_tab == *tab {
            active_style
        } else {
            muted
        };
        spans.push(ratatui::text::Span::styled(label, style));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn render_mailbox_body(buf: &mut Buffer, area: Rect, app: &App) {
    if app.selected_agent_id().is_none() {
        let muted = Style::default().fg(app.capabilities.muted());
        Paragraph::new("(select an agent)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(area, buf);
        return;
    }

    let rows = app.mailbox.rows(app.mailbox_tab);
    if rows.is_empty() {
        let muted = Style::default().fg(app.capabilities.muted());
        Paragraph::new(app.mailbox_tab.empty_hint())
            .style(muted)
            .alignment(Alignment::Center)
            .render(area, buf);
        return;
    }

    // Tail to whatever fits — same shape as the detail pane.
    let cap = area.height as usize;
    let start = rows.len().saturating_sub(cap);
    let lines: Vec<Line<'_>> = rows[start..]
        .iter()
        .map(|r| Line::raw(render_row(r)))
        .collect();
    Paragraph::new(lines).render(area, buf);
}

fn pane_block<'a>(title: &'a str, focused: bool, app: &App) -> Block<'a> {
    let border = if focused {
        Style::default()
            .fg(app.capabilities.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.capabilities.muted())
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border)
}
