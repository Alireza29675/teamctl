//! Bottom statusline — `·`-separated key hints contextual to the
//! focused pane, with the always-visible `· t tutorial` hint pinned
//! to the right per SPEC §4. Styles inactive hints muted so the
//! contextual ones read as the actionable surface.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;
use crate::triptych::Pane;

pub fn draw(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    Statusline { app }.render(area, f.buffer_mut());
}

pub struct Statusline<'a> {
    pub app: &'a App,
}

impl Widget for Statusline<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let muted = Style::default().fg(self.app.capabilities.muted());

        let left = match self.app.focused_pane {
            Pane::Roster => "/ search · ⏎ open · @ send · q quit",
            Pane::Detail => "/ filter · w wall · @ send · esc back · q quit",
            Pane::Mailbox => "Tab tabs · ⏎ open · ! broadcast · q quit",
        };

        // Always-visible right-anchor hint per SPEC §4.
        let right = "? help · t tutorial";

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(right.len() as u16 + 1),
            ])
            .split(area);

        Paragraph::new(left)
            .style(muted)
            .alignment(Alignment::Left)
            .render(cols[0], buf);
        Paragraph::new(right)
            .style(muted)
            .alignment(Alignment::Right)
            .render(cols[1], buf);
    }
}
