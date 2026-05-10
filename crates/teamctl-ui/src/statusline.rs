//! Bottom statusline — `·`-separated key hints contextual to the
//! focused pane, with the always-visible `· t tutorial` hint pinned
//! to the right per SPEC §4. Styles inactive hints muted so the
//! contextual ones read as the actionable surface.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
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
        // T-108 stream-keys: when stream-mode is active the
        // statusline becomes the load-bearing visual indicator —
        // bright reversed banner across the whole row, "STREAM-KEYS
        // → <agent>  ·  Esc to exit". The bright reversed style
        // matches the approvals stripe affordance so the operator
        // reads it as a sticky modal warning even in monochrome
        // terminals where colour alone wouldn't carry.
        if matches!(self.app.stage, crate::app::Stage::StreamKeys) {
            let target = self
                .app
                .selected_agent_id()
                .unwrap_or_else(|| "<no agent>".into());
            let banner = format!(
                "● STREAM-KEYS → {target}   keystrokes forwarding to tmux pane   ·   Esc to exit"
            );
            let style = Style::default()
                .fg(self.app.capabilities.accent())
                .add_modifier(Modifier::REVERSED | Modifier::BOLD);
            Paragraph::new(banner)
                .style(style)
                .alignment(Alignment::Left)
                .render(area, buf);
            return;
        }
        // T-074 bug 7: the Tab pane-cycle chord is the load-bearing
        // navigation primitive — operators who don't discover it get
        // stranded in whichever pane Tab dropped them into. Pin it
        // as the first segment of the statusline in *every* pane,
        // styled bold + accented so it stands out from the muted
        // contextual hints.
        let tab_hint = Span::styled(
            "Tab cycle panes",
            Style::default()
                .fg(self.app.capabilities.accent())
                .add_modifier(Modifier::BOLD),
        );
        let sep = Span::styled("  ·  ", muted);

        let contextual = match self.app.focused_pane {
            Pane::Roster => "/ search · ⏎ open · @ send · q quit",
            // T-108: surface the Ctrl+E entry chord when detail is
            // focused so stream-keys is discoverable without
            // opening the help overlay.
            Pane::Detail => "Ctrl+E stream keys · / filter · w wall · @ send · q quit",
            Pane::Mailbox => "← / → tabs · ⏎ open · ! broadcast · q quit",
        };

        let left = Line::from(vec![tab_hint, sep, Span::styled(contextual, muted)]);

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
            .alignment(Alignment::Left)
            .render(cols[0], buf);
        Paragraph::new(right)
            .style(muted)
            .alignment(Alignment::Right)
            .render(cols[1], buf);
    }
}
