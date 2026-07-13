//! Interactive `teamctl init` picker — a two-pane ratatui screen launched
//! by `teamctl init` (via `teamctl-ui --init-picker`). Branch-first:
//! **Browse a template** vs **Co-design with AI**; choosing Browse opens a
//! list ↔ live-detail view of a precomputed catalog, with a team-shape
//! preview (the reporting tree + per-agent capability counts) drawn from
//! `team_core::preview`. `teamctl` owns template parsing and hands this
//! binary ready-to-render catalog entries over the versioned JSON contract.
//!
//! Self-contained on purpose: it shares teamctl-ui's theme + terminal
//! lifecycle but NOT the dashboard `app::run` loop, which is wired to tmux
//! panes / mailbox.db / a file-watcher. This is a small standalone app.
//!
//! The binary entry (`--init-picker` in `main.rs`) renders to **stderr** and
//! prints one `PickerResponse` JSON line to **stdout**, so `teamctl init`
//! can capture the selection while the UI still shows on the operator's
//! terminal (the fzf pattern). `run_standalone` owns that stderr terminal
//! lifecycle.

use std::io;
use std::panic;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use team_core::preview::{PickerCatalogEntry, PickerResponse, ShapeKind, ShapeRow};

use crate::theme::{detect_capabilities, Capabilities};

/// The "t" lifted from teamctl-ui's splash wordmark (figlet isometric4) —
/// shown atop the start screen so it matches the glyph `teamctl ui` opens
/// with. Leading whitespace is load-bearing (the 3D slant); trailing is
/// trimmed and the mark is rendered left-aligned in a centered column.
const WORDMARK_T: &str = r"   ___
  /\  \
  \:\  \
   \:\  \
   /::\  \
  /:/\:\__\
 /:/  \/__/
/:/  /
\/__/";

/// What the picker resolves to. The binary entry maps this to stdout + an
/// exit code that `teamctl init` consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A machine-readable selection for the parent `teamctl init` process.
    Selected(PickerResponse),
    /// Esc / `q` / Ctrl-C without a choice.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Branch,
    Browse,
}

/// The picker's full state. Deterministic and terminal-free, so
/// `render_to_buffer` can snapshot any screen without a real terminal.
pub struct PickerState {
    caps: Capabilities,
    screen: Screen,
    branch_idx: usize, // 0 = Browse, 1 = Co-design
    entries: Vec<PickerCatalogEntry>,
    list_idx: usize,
    /// Selected row in the create-mode modal (`0` = as-is, `1` = customize).
    create_idx: Option<usize>,
}

impl PickerState {
    /// Build a picker over the given catalog entries, starting on the
    /// branch screen.
    pub fn new(caps: Capabilities, entries: Vec<PickerCatalogEntry>) -> Self {
        Self {
            caps,
            screen: Screen::Branch,
            branch_idx: 0,
            entries,
            list_idx: 0,
            create_idx: None,
        }
    }

    /// Jump straight to the Browse screen (used by snapshot tests).
    pub fn browsing(mut self) -> Self {
        self.screen = Screen::Browse;
        self
    }

    /// Handle one key; returns `Some(outcome)` when the picker should exit.
    fn on_key(&mut self, code: KeyCode) -> Option<Outcome> {
        if let Some(idx) = self.create_idx.as_mut() {
            return match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *idx = idx.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *idx = (*idx + 1).min(1);
                    None
                }
                KeyCode::Enter => {
                    let key = self.entries.get(self.list_idx)?.key.clone();
                    let response = if *idx == 0 {
                        PickerResponse::Create { key }
                    } else {
                        PickerResponse::Customize { key }
                    };
                    Some(Outcome::Selected(response))
                }
                KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                    self.create_idx = None;
                    None
                }
                KeyCode::Char('q') => Some(Outcome::Cancelled),
                _ => None,
            };
        }

        match self.screen {
            Screen::Branch => match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.branch_idx = self.branch_idx.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.branch_idx = (self.branch_idx + 1).min(1);
                    None
                }
                KeyCode::Enter => {
                    if self.branch_idx == 0 {
                        self.screen = Screen::Browse;
                        None
                    } else {
                        Some(Outcome::Selected(PickerResponse::CoDesign))
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => Some(Outcome::Cancelled),
                _ => None,
            },
            Screen::Browse => match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.list_idx = self.list_idx.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.list_idx + 1 < self.entries.len() {
                        self.list_idx += 1;
                    }
                    None
                }
                KeyCode::Enter => {
                    if self.entries.get(self.list_idx).is_some() {
                        self.create_idx = Some(0);
                    }
                    None
                }
                KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                    self.screen = Screen::Branch;
                    None
                }
                KeyCode::Char('q') => Some(Outcome::Cancelled),
                _ => None,
            },
        }
    }

    fn on_event(&mut self, key: KeyEvent) -> Option<Outcome> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Outcome::Cancelled);
        }
        self.on_key(key.code)
    }
}

/// Render the current screen into `buf` — the single rendering entry point,
/// shared by the live loop (`draw`) and the snapshot helper.
fn render(state: &PickerState, area: Rect, buf: &mut Buffer) {
    // The full branch/browser/modal layouts need room for fixed-height art,
    // borders, and two readable options. Ratatui's constrained rows can sit
    // just outside a 1-row buffer, so render a compact resize prompt instead
    // of risking an out-of-bounds write on a tiny terminal.
    if area.width < 48 || area.height < 12 {
        Paragraph::new("Terminal too small\nResize to continue")
            .style(Style::default().fg(state.caps.muted()))
            .alignment(Alignment::Center)
            .render(area, buf);
        return;
    }
    match state.screen {
        Screen::Branch => render_branch(state, area, buf),
        Screen::Browse => render_browse(state, area, buf),
    }
    if state.create_idx.is_some() {
        render_create_modal(state, area, buf);
    }
}

/// Snapshot helper: render `state` into a fresh `width × height` buffer.
pub fn render_to_buffer(state: &PickerState, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render(state, area, &mut buf);
    buf
}

fn draw(frame: &mut Frame, state: &PickerState) {
    render(state, frame.area(), frame.buffer_mut());
}

fn render_branch(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let accent = Style::default()
        .fg(state.caps.accent())
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(state.caps.muted());

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // top spacer
            Constraint::Length(9), // "t" splash glyph
            Constraint::Length(1), // gap
            Constraint::Length(1), // title
            Constraint::Length(1), // gap
            Constraint::Length(1), // option 0
            Constraint::Length(1), // option 1
            Constraint::Length(1), // gap
            Constraint::Length(1), // hint
            Constraint::Min(0),    // bottom spacer
        ])
        .split(area);

    // Render the splash glyph left-aligned inside a horizontally-centered
    // column so its diagonal stays intact (per-line centering would skew it).
    let mark_w = WORDMARK_T
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let mark_area = Rect {
        x: area.x + area.width.saturating_sub(mark_w) / 2,
        y: rows[1].y,
        width: mark_w.min(area.width),
        height: rows[1].height,
    };
    Paragraph::new(WORDMARK_T)
        .style(accent)
        .render(mark_area, buf);

    Paragraph::new("Create a new team")
        .style(accent)
        .alignment(Alignment::Center)
        .render(rows[3], buf);

    let option = |idx: usize, label: &str, desc: &str| -> Line<'static> {
        let selected = state.branch_idx == idx;
        let marker = if selected { "▸ " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(state.caps.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label}"), style),
            Span::styled(
                format!("   {desc}"),
                Style::default().fg(state.caps.muted()),
            ),
        ])
    };

    // Both options share one left edge: render them left-aligned inside a
    // horizontally-centered column so the labels line up, instead of each
    // line centering on its own (which reads ragged-left).
    let opt0 = option(0, "Browse a template", "pick from ready-made teams");
    let opt1 = option(1, "Co-design with AI", "let Claude Code shape it with you");
    let col_w = (opt0.width().max(opt1.width()) as u16).min(area.width);
    let col_x = area.x + area.width.saturating_sub(col_w) / 2;
    let line_area = |row: Rect| Rect {
        x: col_x,
        y: row.y,
        width: col_w,
        height: 1,
    };
    Paragraph::new(opt0).render(line_area(rows[5]), buf);
    Paragraph::new(opt1).render(line_area(rows[6]), buf);

    Paragraph::new("↑/↓ select · Enter choose · Esc/q cancel")
        .style(muted)
        .alignment(Alignment::Center)
        .render(rows[8], buf);
}

fn render_browse(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(vchunks[0]);

    render_list(state, panes[0], buf);
    render_detail(state, panes[1], buf);

    Paragraph::new("↑/↓ select · Enter options · Esc back · q cancel")
        .style(Style::default().fg(state.caps.muted()))
        .alignment(Alignment::Center)
        .render(vchunks[1], buf);
}

fn render_list(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .title(" Templates ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.caps.muted()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    block.render(area, buf);

    if state.entries.is_empty() {
        Paragraph::new("(no templates found)")
            .style(Style::default().fg(state.caps.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }

    let lines: Vec<Line<'_>> = state
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let selected = i == state.list_idx;
            let marker = if selected { "▸ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(state.caps.accent())
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::styled(format!("{marker}{}", e.name), style)
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

fn render_detail(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let entry = state.entries.get(state.list_idx);
    let title = entry.map(|e| format!(" {} ", e.name)).unwrap_or_default();
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.caps.accent()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    block.render(area, buf);

    let Some(entry) = entry else { return };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if !entry.description.is_empty() {
        lines.push(Line::styled(
            entry.description.clone(),
            Style::default().fg(state.caps.muted()),
        ));
        lines.push(Line::raw(""));
    }
    let c = entry.counts;
    lines.push(Line::styled(
        format!(
            "{} · {} · {} · {} · {}",
            plural(c.agents, "agent"),
            plural(c.subagents, "sub-agent"),
            plural(c.skills, "skill"),
            plural(c.hooks, "hook"),
            plural(c.mcps, "mcp"),
        ),
        Style::default().fg(state.caps.muted()),
    ));
    lines.push(Line::raw(""));
    lines.extend(shape_to_lines(&entry.rows, state.caps));

    // No wrap: the tree lines are structured, so on a narrow terminal we
    // truncate at the pane edge rather than wrap a descriptor onto a second
    // line without its tree prefix (which reads as broken).
    Paragraph::new(lines).render(inner, buf);
}

fn render_create_modal(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let selected = state.create_idx.unwrap_or_default();
    let popup = centered_rect(68, 10, area);
    Clear.render(popup, buf);

    let block = Block::default()
        .title(" Create template ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.caps.accent()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    block.render(popup, buf);

    let entry_name = state
        .entries
        .get(state.list_idx)
        .map(|entry| entry.name.as_str())
        .unwrap_or("this template");
    let option = |idx: usize, label: &str, detail: &str| {
        let is_selected = selected == idx;
        let style = if is_selected {
            Style::default()
                .fg(state.caps.accent())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(
                format!("{} {label}", if is_selected { "▸" } else { " " }),
                style,
            ),
            Span::styled(
                format!("  {detail}"),
                Style::default().fg(state.caps.muted()),
            ),
        ])
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);
    Paragraph::new(format!("How should teamctl create {entry_name}?"))
        .style(Style::default().add_modifier(Modifier::BOLD))
        .render(rows[0], buf);
    Paragraph::new(option(0, "Create as-is", "scaffold the ready-made team")).render(rows[2], buf);
    Paragraph::new(option(
        1,
        "Create and customize",
        "scaffold, then open Claude Code to customize",
    ))
    .render(rows[3], buf);
    Paragraph::new("↑/↓ select · Enter confirm · Esc back · q cancel")
        .style(Style::default().fg(state.caps.muted()))
        .alignment(Alignment::Center)
        .render(rows[6], buf);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Turn the front-end-agnostic `ShapeRow`s into styled tree lines, mapping
/// `is_last` back to the `└──`/`├──` connectors and drawing continuation
/// columns for ancestor levels. Mirrors init.rs's box-drawing tree.
fn shape_to_lines(rows: &[ShapeRow], caps: Capabilities) -> Vec<Line<'static>> {
    let mut last_at: Vec<bool> = Vec::new();
    let mut out: Vec<Line<'static>> = Vec::new();
    for r in rows {
        if matches!(r.kind, ShapeKind::Root) {
            out.push(Line::styled(
                r.label.clone(),
                Style::default()
                    .fg(caps.accent())
                    .add_modifier(Modifier::BOLD),
            ));
            last_at = vec![true];
            continue;
        }
        let depth = r.depth as usize;
        // Continuation columns for ancestor depths 1..depth.
        let mut prefix = String::new();
        for d in 1..depth {
            let ancestor_last = last_at.get(d).copied().unwrap_or(true);
            prefix.push_str(if ancestor_last { "    " } else { "│   " });
        }
        let connector = if r.is_last {
            "└── "
        } else {
            "├── "
        };
        if last_at.len() <= depth {
            last_at.resize(depth + 1, true);
        }
        last_at[depth] = r.is_last;

        let mut spans = vec![
            Span::styled(
                format!("{prefix}{connector}"),
                Style::default().fg(caps.muted()),
            ),
            Span::styled(r.label.clone(), Style::default().fg(caps.accent())),
        ];
        if !r.descriptor.is_empty() {
            spans.push(Span::styled(
                format!("  {}", r.descriptor),
                Style::default().fg(caps.muted()),
            ));
        }
        out.push(Line::from(spans));
    }
    out
}

/// Drive the picker against `terminal` until the operator chooses or
/// cancels.
fn run<B: Backend>(terminal: &mut Terminal<B>, mut state: PickerState) -> Result<Outcome> {
    loop {
        terminal.draw(|f| draw(f, &state))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if let Some(outcome) = state.on_event(key) {
                return Ok(outcome);
            }
        }
    }
}

/// Binary entry: own the **stderr** terminal lifecycle (so stdout stays
/// clean for the JSON response), run the picker, and restore the terminal on
/// every exit path including panics.
pub fn run_standalone(entries: Vec<PickerCatalogEntry>) -> Result<Outcome> {
    let caps = detect_capabilities();
    let state = PickerState::new(caps, entries);

    install_panic_hook();
    enter_terminal()?;
    // Everything past raw-mode-on runs inside this closure so the
    // unconditional `leave_terminal()` below restores the terminal on
    // EVERY exit path — a `run` error, a `Terminal::new` failure, or a
    // clean return. Panics are caught by the hook, which calls the same
    // infallible teardown.
    let result = (move || {
        let backend = CrosstermBackend::new(io::stderr());
        let mut terminal = Terminal::new(backend)?;
        let outcome = run(&mut terminal, state);
        let _ = terminal.show_cursor();
        outcome
    })();
    leave_terminal();
    result
}

fn enter_terminal() -> Result<()> {
    enable_raw_mode()?;
    // If the alternate-screen step fails, undo raw mode before bailing so
    // the caller never returns with the shell stranded in raw mode.
    if let Err(e) = execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    Ok(())
}

/// Best-effort, unconditional teardown: every step runs regardless of an
/// earlier failure, so `disable_raw_mode()` always fires (the step that
/// actually un-wedges the operator's shell). Safe on any exit path,
/// including the panic hook.
fn leave_terminal() {
    let _ = execute!(io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        leave_terminal();
        original(info);
    }));
}

/// Pluralize a count for the detail headline: `1 hook`, `2 hooks`, `0 mcps`.
fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_to_string(buf: &Buffer) -> String {
        let a = buf.area();
        let mut out = String::new();
        for y in 0..a.height {
            for x in 0..a.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn mono() -> Capabilities {
        Capabilities {
            color: crate::theme::ColorMode::Monochrome,
        }
    }

    /// A deterministic two-manager / one-worker team for the browse snapshot.
    fn fixture_entry() -> PickerCatalogEntry {
        PickerCatalogEntry {
            key: "product-team".into(),
            name: "Product Team".into(),
            description: "product discovery while an engineering team builds".into(),
            rows: vec![
                ShapeRow {
                    depth: 0,
                    kind: ShapeKind::Root,
                    label: "You".into(),
                    descriptor: String::new(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "Product Manager".into(),
                    descriptor: "Claude Code · Opus 4.8 · 2×a 0×s 0×h 0×m".into(),
                    is_last: false,
                },
                ShapeRow {
                    depth: 2,
                    kind: ShapeKind::Worker,
                    label: "Engineer (Claude)".into(),
                    descriptor: "Claude Code · Sonnet 4.6 · 6×a 0×s 1×h 0×m".into(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "Engineering Manager".into(),
                    descriptor: "Claude Code · Opus 4.8 · 2×a 0×s 0×h 0×m".into(),
                    is_last: true,
                },
            ],
            counts: team_core::preview::PreviewCounts {
                agents: 3,
                subagents: 10,
                skills: 0,
                hooks: 1,
                mcps: 0,
            },
        }
    }

    #[test]
    fn branch_screen_snapshot() {
        let state = PickerState::new(mono(), vec![]);
        insta::assert_snapshot!(buf_to_string(&render_to_buffer(&state, 100, 24)));
    }

    #[test]
    fn browse_screen_snapshot() {
        let state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        insta::assert_snapshot!(buf_to_string(&render_to_buffer(&state, 100, 20)));
    }

    #[test]
    fn browse_is_immediate_over_precomputed_catalog() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]);
        assert_eq!(state.on_key(KeyCode::Enter), None); // Branch → Browse
        let out = buf_to_string(&render_to_buffer(&state, 100, 16));
        assert!(
            out.contains("Product Team"),
            "catalog should render immediately:\n{out}"
        );
        assert!(
            !out.contains("Fetching templates"),
            "no fake loading state:\n{out}"
        );
    }

    #[test]
    fn enter_on_browse_opens_create_modal_with_as_is_default() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        assert_eq!(state.on_key(KeyCode::Enter), None);
        assert_eq!(state.create_idx, Some(0));
    }

    #[test]
    fn enter_on_create_modal_returns_create_as_is_for_current_entry() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        state.on_key(KeyCode::Enter);
        assert_eq!(
            state.on_key(KeyCode::Enter),
            Some(Outcome::Selected(PickerResponse::Create {
                key: "product-team".into()
            }))
        );
    }

    #[test]
    fn down_then_enter_on_create_modal_returns_customize_for_current_entry() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        state.on_key(KeyCode::Enter);
        state.on_key(KeyCode::Down);
        assert_eq!(
            state.on_key(KeyCode::Enter),
            Some(Outcome::Selected(PickerResponse::Customize {
                key: "product-team".into()
            }))
        );
    }

    #[test]
    fn escape_closes_create_modal_without_leaving_browse() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        state.on_key(KeyCode::Enter);
        assert_eq!(state.on_key(KeyCode::Esc), None);
        assert_eq!(state.create_idx, None);
        assert_eq!(state.screen, Screen::Browse);
    }

    #[test]
    fn modal_navigation_does_not_move_template_cursor() {
        let mut second = fixture_entry();
        second.key = "second".into();
        second.name = "Second".into();
        let mut state = PickerState::new(mono(), vec![fixture_entry(), second]).browsing();
        state.on_key(KeyCode::Enter);
        state.on_key(KeyCode::Down);
        assert_eq!(state.list_idx, 0);
        assert_eq!(state.create_idx, Some(1));
    }

    #[test]
    fn enter_on_empty_catalog_does_not_open_modal() {
        let mut state = PickerState::new(mono(), vec![]).browsing();
        assert_eq!(state.on_key(KeyCode::Enter), None);
        assert_eq!(state.create_idx, None);
    }

    #[test]
    fn create_action_modal_snapshot() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        state.on_key(KeyCode::Enter);
        insta::assert_snapshot!(buf_to_string(&render_to_buffer(&state, 100, 20)));
    }

    #[test]
    fn co_design_branch_returns_codesign() {
        let mut state = PickerState::new(mono(), vec![]);
        state.on_key(crossterm::event::KeyCode::Down); // → Co-design
        assert_eq!(
            state.on_key(KeyCode::Enter),
            Some(Outcome::Selected(PickerResponse::CoDesign))
        );
        assert_eq!(state.create_idx, None, "co-design bypasses create modal");
    }

    #[test]
    fn esc_on_branch_cancels() {
        let mut state = PickerState::new(mono(), vec![]);
        assert_eq!(
            state.on_key(crossterm::event::KeyCode::Esc),
            Some(Outcome::Cancelled)
        );
    }

    #[test]
    fn ctrl_c_cancels_from_any_screen() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        state.on_key(KeyCode::Enter); // modal open
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.on_event(key), Some(Outcome::Cancelled));
    }

    #[test]
    fn picker_and_modal_do_not_panic_on_tiny_terminals() {
        for (width, height) in [(0, 0), (1, 1), (20, 5), (40, 8)] {
            let state = PickerState::new(mono(), vec![fixture_entry()]);
            let _ = render_to_buffer(&state, width, height);

            let mut modal = PickerState::new(mono(), vec![fixture_entry()]).browsing();
            modal.on_key(KeyCode::Enter);
            let _ = render_to_buffer(&modal, width, height);
        }
    }
}
