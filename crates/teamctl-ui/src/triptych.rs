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

/// Top-level layout selector for the main view (Stage::Triptych).
/// PR-UI-1..5 used the Triptych shape exclusively; PR-UI-6 adds
/// Wall (orchestrator overview, up to 4 tiles + scroll) and
/// MailboxFirst (channel-feed centric for cross-team triage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainLayout {
    Triptych,
    Wall,
    MailboxFirst,
}

impl MainLayout {
    /// `Ctrl+W` (or standalone `w` from the SPEC chord map)
    /// toggles between Triptych ↔ Wall.
    pub fn toggle_wall(self) -> Self {
        if matches!(self, MainLayout::Wall) {
            MainLayout::Triptych
        } else {
            MainLayout::Wall
        }
    }

    /// `Ctrl+M` toggles between Triptych ↔ MailboxFirst.
    pub fn toggle_mailbox_first(self) -> Self {
        if matches!(self, MainLayout::MailboxFirst) {
            MainLayout::Triptych
        } else {
            MainLayout::MailboxFirst
        }
    }
}

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

    /// `Shift+Tab` cycles backward — roster → mailbox → detail →
    /// roster. Closes the no-easy-exit-from-mailbox UX gap PR-UI-3
    /// surfaced: operator Tabs into mailbox, then with Shift+Tab
    /// they back out cleanly without the `q`-confirm round-trip.
    pub fn prev(self) -> Self {
        match self {
            Pane::Roster => Pane::Mailbox,
            Pane::Detail => Pane::Roster,
            Pane::Mailbox => Pane::Detail,
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
        // PR-UI-4: the approvals stripe takes one line at the top
        // when there's at least one pending approval. The
        // `stripe_visible` const PR-UI-1 scaffolded as `false` is
        // now `app.has_pending_approvals()`.
        let stripe_visible = self.app.has_pending_approvals();
        let body = if stripe_visible {
            let v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);
            render_approvals_stripe(buf, v[0], self.app);
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

fn render_approvals_stripe(buf: &mut Buffer, area: Rect, app: &App) {
    let n = app.pending_approvals.len();
    let plural = if n == 1 { "" } else { "s" };
    let text = format!("⚠  approvals: {n} pending{plural} — `a` to review");
    // Bright accent + reversed for the stripe — same affordance
    // pattern as the focused-pane border, applied to a full row so
    // the warning reads in any colour mode.
    let style = Style::default()
        .fg(app.capabilities.accent())
        .add_modifier(Modifier::REVERSED | Modifier::BOLD);
    Paragraph::new(text)
        .style(style)
        .alignment(Alignment::Left)
        .render(area, buf);
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
    let focused_pane = app.focused_pane == Pane::Detail;
    let title = match app
        .selected_agent
        .and_then(|i| app.team.agents.get(i))
        .map(|a| a.id.as_str())
    {
        Some(id) => format!("DETAIL · {id}"),
        None => "DETAIL".to_string(),
    };
    let outer_block = pane_block(&title, focused_pane, app);
    let inner = outer_block.inner(area);
    outer_block.render(area, buf);

    if app.selected_agent.is_none() || app.team.agents.is_empty() {
        let muted = Style::default().fg(app.capabilities.muted());
        Paragraph::new("(select an agent on the left to follow its session)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }

    // PR-UI-7 fixup (qa Gap D): when `detail_splits` is non-empty
    // the detail pane subdivides — primary cell shows the focused
    // agent, additional cells show each split's agent. Operators
    // see the actual visual effect of `Ctrl+|` / `Ctrl+-`.
    if !app.detail_splits.is_empty() {
        render_detail_splits(buf, inner, app);
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
    // T-074 bug 3: parse the ANSI escape sequences captured by
    // `tmux capture-pane -e` into styled spans. `Line::raw` would
    // render the escapes as literal `\x1b[...` garbage; `into_text`
    // turns SGR codes (colours, bold, dim, …) into ratatui spans
    // so the agent's terminal output renders coloured. Lines that
    // contain no ANSI degrade gracefully to plain spans.
    use ansi_to_tui::IntoText;
    let lines: Vec<Line<'_>> = app.detail_buffer[start..]
        .iter()
        .flat_map(|s| match s.as_bytes().into_text() {
            Ok(text) => text.lines.into_iter().collect::<Vec<_>>(),
            Err(_) => vec![Line::raw(s.clone())],
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

/// Subdivide the detail-pane area when `detail_splits` is
/// non-empty. Composition (qa Gap D fixup):
///
/// - Cell 0 always shows the focused agent (the original detail
///   stream); cells 1..=N show each split's agent in order.
/// - The operator's mental model is "vertical adds a column,
///   horizontal adds a row." We honour that by folding vertical
///   splits into columns first, then horizontal splits subdivide
///   each column. With all-vertical or all-horizontal splits the
///   layout is straightforward; with a mix the columns grow
///   left-to-right and the horizontal splits stack within their
///   column.
/// - Each cell renders the agent's id + state glyph in the title
///   bar and the focused agent's `detail_buffer` lines as content.
///   Non-focused splits show a `(focus this split to stream)`
///   placeholder — multi-stream pane captures land in T-068
///   alongside the per-tile Wall captures.
/// - The focused split (per `app.selected_split`) gets the accent
///   focus-ring border; others get the muted border.
fn render_detail_splits(buf: &mut Buffer, area: Rect, app: &App) {
    use ratatui::layout::Direction as Dir;

    // Build the cell list: [focused, split_0, split_1, ...].
    // Each cell carries (agent_id, orientation_hint, is_focused_split).
    // `orientation_hint` for the focused agent defaults to Vertical
    // so the first split's chord choice drives the layout.
    let focused_id = app
        .selected_agent_id()
        .unwrap_or_else(|| "<no agent>".into());
    let mut cells: Vec<(String, crate::app::SplitOrientation, bool)> = Vec::new();
    cells.push((
        focused_id,
        // Match whatever the first split orientation is (or Vertical
        // if no splits — the no-splits path is short-circuited
        // above this fn's caller).
        app.detail_splits
            .first()
            .map(|(_, o)| *o)
            .unwrap_or(crate::app::SplitOrientation::Vertical),
        app.selected_split == 0 && app.focused_pane == Pane::Detail,
    ));
    for (i, (id, orientation)) in app.detail_splits.iter().enumerate() {
        cells.push((
            id.clone(),
            *orientation,
            app.selected_split == i + 1 && app.focused_pane == Pane::Detail,
        ));
    }

    // Group cells into columns: a Vertical split starts a new
    // column; Horizontal splits stack within the current column.
    let mut columns: Vec<Vec<usize>> = vec![vec![0]];
    for (idx, (_, orientation, _)) in cells.iter().enumerate().skip(1) {
        match orientation {
            crate::app::SplitOrientation::Vertical => columns.push(vec![idx]),
            crate::app::SplitOrientation::Horizontal => {
                columns.last_mut().expect("seed column").push(idx);
            }
        }
    }

    let col_count = columns.len();
    let col_constraints: Vec<Constraint> = (0..col_count)
        .map(|_| Constraint::Ratio(1, col_count as u32))
        .collect();
    let col_areas = ratatui::layout::Layout::default()
        .direction(Dir::Horizontal)
        .constraints(col_constraints)
        .split(area);

    for (col_idx, col_cells) in columns.iter().enumerate() {
        let col_area = col_areas[col_idx];
        let row_count = col_cells.len();
        let row_constraints: Vec<Constraint> = (0..row_count)
            .map(|_| Constraint::Ratio(1, row_count as u32))
            .collect();
        let row_areas = ratatui::layout::Layout::default()
            .direction(Dir::Vertical)
            .constraints(row_constraints)
            .split(col_area);
        for (row_idx, &cell_idx) in col_cells.iter().enumerate() {
            let cell_area = row_areas[row_idx];
            let (agent_id, _, is_focused_split) = &cells[cell_idx];
            render_split_cell(buf, cell_area, app, agent_id, *is_focused_split);
        }
    }
}

fn render_split_cell(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    agent_id: &str,
    is_focused_split: bool,
) {
    let ascii = matches!(app.capabilities.color, ColorMode::Monochrome);
    let glyph = app
        .team
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|info| crate::data::state_glyph(info, ascii))
        .unwrap_or("?");
    let title = format!(" {glyph} {agent_id} ");
    let border = if is_focused_split {
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
    let inner = block.inner(area);
    block.render(area, buf);

    // Only the focused split streams the live detail buffer.
    // Non-focused splits show the placeholder — multi-stream
    // captures land in T-068 alongside Wall's per-tile streaming.
    let muted = Style::default().fg(app.capabilities.muted());
    if !is_focused_split {
        Paragraph::new("(focus this split to stream)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    if app.detail_buffer.is_empty() {
        Paragraph::new("(no scrollback yet)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
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
