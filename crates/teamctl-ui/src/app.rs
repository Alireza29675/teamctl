//! App state and the top-level run loop.
//!
//! Three stages today: `Splash` (figlet logo for ~3s or until first
//! key), `Triptych` (the default empty-state read view), and
//! `QuitConfirm` (a modal asking "really?"). Subsequent stacked PRs
//! bolt on real data subscribers, more modals, and the layout
//! variants from SPEC §3 — those wire in by adding `Stage` variants
//! and dispatching from `draw`/`handle_event`, no rearchitecting.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use crate::splash;
use crate::statusline;
use crate::theme::{detect_capabilities, Capabilities};
use crate::triptych::{self, Pane};
use crate::tutorial;

const SPLASH_AUTO_DISMISS: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Splash,
    Triptych,
    QuitConfirm,
}

pub struct App {
    pub stage: Stage,
    /// Tracked so QuitConfirm can return to whichever stage opened it.
    pub previous_stage: Stage,
    pub focused_pane: Pane,
    pub team_name: String,
    pub agent_count: usize,
    pub version: &'static str,
    pub capabilities: Capabilities,
    pub splash_started: Instant,
    pub running: bool,
    /// First-launch detection — when the marker file exists, future
    /// stacked-PRs (PR-UI-7) skip the tutorial after splash. PR-UI-1
    /// only reads the flag; nothing routes off it yet.
    pub tutorial_completed: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            stage: Stage::Splash,
            previous_stage: Stage::Splash,
            focused_pane: Pane::Roster,
            // Mock values until PR-UI-2 wires the compose subscriber.
            team_name: "(no team loaded)".into(),
            agent_count: 0,
            version: env!("CARGO_PKG_VERSION"),
            capabilities: detect_capabilities(),
            splash_started: Instant::now(),
            running: true,
            tutorial_completed: tutorial::is_completed(),
        }
    }

    pub fn dismiss_splash(&mut self) {
        if matches!(self.stage, Stage::Splash) {
            self.stage = Stage::Triptych;
            self.previous_stage = Stage::Triptych;
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    pub fn enter_quit_confirm(&mut self) {
        self.previous_stage = self.stage;
        self.stage = Stage::QuitConfirm;
    }

    pub fn cancel_quit(&mut self) {
        self.stage = self.previous_stage;
    }

    pub fn confirm_quit(&mut self) {
        self.running = false;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn run<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut app = App::new();
    while app.running {
        terminal.draw(|f| draw(f, &app))?;
        if event::poll(POLL_INTERVAL)? {
            handle_event(&mut app, event::read()?);
        }
        if matches!(app.stage, Stage::Splash) && app.splash_started.elapsed() >= SPLASH_AUTO_DISMISS
        {
            app.dismiss_splash();
        }
    }
    Ok(())
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    match app.stage {
        Stage::Splash => splash::draw(f, app),
        Stage::Triptych => draw_main(f, area, app),
        Stage::QuitConfirm => {
            draw_main(f, area, app);
            draw_quit_confirm(f, area);
        }
    }
}

fn draw_main(f: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    triptych::draw(f, chunks[0], app);
    statusline::draw(f, chunks[1], app);
}

fn draw_quit_confirm(f: &mut Frame<'_>, area: Rect) {
    let popup_w = 36u16.min(area.width.saturating_sub(2));
    let popup_h = 5u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    let buf = f.buffer_mut();
    Clear.render(popup, buf);
    Paragraph::new("Quit teamctl-ui?  [y / n]")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("confirm"))
        .render(popup, buf);
}

fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn handle_event(app: &mut App, ev: Event) {
    match ev {
        Event::Key(k) if k.kind == KeyEventKind::Press => match app.stage {
            Stage::Splash => app.dismiss_splash(),
            Stage::Triptych => match k.code {
                KeyCode::Char('q') => app.enter_quit_confirm(),
                KeyCode::Tab => app.cycle_focus(),
                _ => {}
            },
            Stage::QuitConfirm => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.confirm_quit(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_quit(),
                _ => {}
            },
        },
        Event::Resize(_, _) => {
            // ratatui redraws on the next loop iteration; nothing to do.
        }
        _ => {}
    }
}

/// Render the entire UI into a `Buffer` at fixed size — used by the
/// snapshot tests. Mirrors `draw` exactly but doesn't require a
/// `Terminal`. Update both in lockstep when adding new stages.
pub fn render_to_buffer(app: &App, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    match app.stage {
        Stage::Splash => splash::Splash { app }.render(area, &mut buf),
        Stage::Triptych => render_main(app, area, &mut buf),
        Stage::QuitConfirm => {
            render_main(app, area, &mut buf);
            render_quit_confirm(area, &mut buf);
        }
    }
    buf
}

fn render_main(app: &App, area: Rect, buf: &mut Buffer) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    triptych::Triptych { app }.render(chunks[0], buf);
    statusline::Statusline { app }.render(chunks[1], buf);
}

fn render_quit_confirm(area: Rect, buf: &mut Buffer) {
    let popup_w = 36u16.min(area.width.saturating_sub(2));
    let popup_h = 5u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    Paragraph::new("Quit teamctl-ui?  [y / n]")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("confirm"))
        .render(popup, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn splash_dismissed_by_any_key() {
        let mut app = App::new();
        assert_eq!(app.stage, Stage::Splash);
        handle_event(&mut app, key(KeyCode::Char(' ')));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn tab_cycles_focus_through_three_panes() {
        let mut app = App::new();
        app.dismiss_splash();
        assert_eq!(app.focused_pane, Pane::Roster);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Detail);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Mailbox);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Roster);
    }

    #[test]
    fn q_opens_confirm_then_n_cancels() {
        let mut app = App::new();
        app.dismiss_splash();
        handle_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.stage, Stage::QuitConfirm);
        handle_event(&mut app, key(KeyCode::Char('n')));
        assert_eq!(app.stage, Stage::Triptych);
        assert!(app.running, "n must not exit");
    }

    #[test]
    fn q_then_y_exits() {
        let mut app = App::new();
        app.dismiss_splash();
        handle_event(&mut app, key(KeyCode::Char('q')));
        handle_event(&mut app, key(KeyCode::Char('y')));
        assert!(!app.running);
    }

    #[test]
    fn esc_cancels_quit_confirm() {
        let mut app = App::new();
        app.dismiss_splash();
        app.enter_quit_confirm();
        handle_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn render_does_not_panic_at_minimal_size() {
        // Anything ≥ a 1×1 buffer must round-trip without panicking;
        // ratatui swallows constraints that exceed the area.
        let app = App::new();
        let _ = render_to_buffer(&app, 20, 8);
    }

    #[test]
    fn render_does_not_panic_at_huge_size() {
        let app = App::new();
        let _ = render_to_buffer(&app, 240, 80);
    }
}
