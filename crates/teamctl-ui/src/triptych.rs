//! Triptych — the default Layout A. Three resizable panes (roster,
//! detail, mailbox) with an Approvals stripe reserved at the top
//! (rendered only when there's something to surface — empty in
//! PR-UI-1) and a focus ring on the active pane.
//!
//! PR-UI-1 ships the layout primitives + empty-state placeholders;
//! real agent / mailbox data lands in PR-UI-2 and PR-UI-3. The pane
//! widths match SPEC §2: roster ~28 cols, mailbox ~32 cols, detail
//! takes the remainder.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::app::App;

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
        // there's a pending approval. PR-UI-1 has no real data, so
        // the stripe is hidden and the panes get the full area.
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

        render_pane(
            buf,
            columns[0],
            "ROSTER",
            "(no agents yet)",
            self.app,
            Pane::Roster,
        );
        render_pane(
            buf,
            columns[1],
            "DETAIL",
            "(no agent selected)",
            self.app,
            Pane::Detail,
        );
        render_pane(
            buf,
            columns[2],
            "MAILBOX",
            "(no mailbox)",
            self.app,
            Pane::Mailbox,
        );
    }
}

fn render_pane(buf: &mut Buffer, area: Rect, title: &str, empty: &str, app: &App, which: Pane) {
    let focused = app.focused_pane == which;
    let border = if focused {
        Style::default()
            .fg(app.capabilities.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.capabilities.muted())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border);
    let muted = Style::default().fg(app.capabilities.muted());
    Paragraph::new(empty)
        .style(muted)
        .alignment(Alignment::Center)
        .block(block)
        .render(area, buf);
}
