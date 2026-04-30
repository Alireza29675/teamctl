//! Splash screen widget — figlet-isometric4 logo, version + team line,
//! help-hint footer. Shown for ~3 seconds at launch (or until a key
//! press) before the Triptych takes over. The art is vendored as a
//! static asset; regenerate with `figlet -f isometric4 teamctl` when
//! the wordmark changes.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;

const SPLASH_ART: &str = include_str!("assets/splash.txt");

pub fn draw(f: &mut ratatui::Frame<'_>, app: &App) {
    Splash { app }.render(f.area(), f.buffer_mut());
}

/// Standalone widget for snapshot tests: rendering into a `Buffer`
/// directly is enough to assert layout without a `Terminal`.
pub struct Splash<'a> {
    pub app: &'a App,
}

impl Widget for Splash<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),     // top spacer
                Constraint::Length(11), // logo (10 lines of art + 1 padding)
                Constraint::Length(1),  // version + team line
                Constraint::Length(1),  // hint line
                Constraint::Min(0),     // bottom spacer
            ])
            .split(area);

        let accent = Style::default()
            .fg(self.app.capabilities.accent())
            .add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(self.app.capabilities.muted());

        Paragraph::new(SPLASH_ART)
            .style(accent)
            .alignment(Alignment::Center)
            .render(chunks[1], buf);

        let count = self.app.team.agents.len();
        let team_line = format!(
            "v{}  ·  {}  ·  {} agent{}",
            self.app.version,
            self.app.team.team_name,
            count,
            if count == 1 { "" } else { "s" }
        );
        Paragraph::new(team_line)
            .alignment(Alignment::Center)
            .render(chunks[2], buf);

        Paragraph::new("Press `?` for help · `t` for tutorial")
            .style(muted)
            .alignment(Alignment::Center)
            .render(chunks[3], buf);
    }
}
