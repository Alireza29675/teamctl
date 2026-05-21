//! Triptych — the default Layout A. Sidebar + right-stack: an
//! Agents column on the left (current sidebar width), with Detail
//! stacked above Mailbox at 50/50 on the right. An Approvals stripe
//! is reserved at the top (rendered only when there's something to
//! surface) and a focus ring on the active pane.
//!
//! The Agents + Detail panes wire to live data:
//! - Agents lists `app.team.agents` with single-cell state glyphs
//!   driven by `data::state_glyph`. Selection is highlighted with
//!   the focus accent.
//! - Detail shows the last-N lines of `app.detail_buffer` (the
//!   tmux capture-pane scrollback for the focused agent), or an
//!   empty-state hint when no agent is selected.
//! - Mailbox stacks below Detail and pulls from the focused
//!   agent's mailbox tab buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::app::App;
use crate::data::{state_glyph, tree_row_meta, AgentInfo, TreeRowMeta};
use crate::mailbox::{render_row, MailboxInputKind, MailboxTab};
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
    /// `Tab` cycles `Agents → Detail → Mailbox → Agents`. With the
    /// sidebar + right-stack geometry that reads spatially as
    /// left → top-right → bottom-right → wrap to left.
    pub fn next(self) -> Self {
        match self {
            Pane::Roster => Pane::Detail,
            Pane::Detail => Pane::Mailbox,
            Pane::Mailbox => Pane::Roster,
        }
    }

    /// `Shift+Tab` cycles backward — `Agents → Mailbox → Detail →
    /// Agents`. Closes the no-easy-exit-from-mailbox UX gap: the
    /// operator Tabs into Mailbox, then Shift+Tab backs out cleanly
    /// without the `q`-confirm round-trip.
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

        // Outer split: Agents sidebar on the left at the same
        // 28-cell width the previous Triptych Roster column used,
        // then a right-hand stack that fills the rest of the row.
        let outer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(28), // agents sidebar
                Constraint::Min(0),     // right-stack
            ])
            .split(body);

        // Inner split inside the right stack: Detail above at 60%,
        // Mailbox below at 40%. The ratio survives terminal-resize
        // events — `Constraint::Ratio` re-applies on every render.
        let right_stack = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(3, 5), Constraint::Ratio(2, 5)])
            .split(outer[1]);

        render_agents(buf, outer[0], self.app);
        render_detail(buf, right_stack[0], self.app);
        render_mailbox(buf, right_stack[1], self.app);
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

fn render_agents(buf: &mut Buffer, area: Rect, app: &App) {
    let focused = app.focused_pane == Pane::Roster;
    let block = pane_block("AGENTS", focused, app);
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
    // T-211: pair each row with tree-render metadata so `agent_line`
    // can draw `├─` / `└─` glyphs for `reports_to` children. Teams
    // with no `reports_to` usage produce all-depth-0 metas and the
    // existing flat-render output is byte-identical (no prefix bytes
    // for depth 0).
    let metas = tree_row_meta(&app.team.agents);
    let lines: Vec<Line<'_>> = app
        .team
        .agents
        .iter()
        .zip(metas.iter())
        .enumerate()
        .map(|(i, (info, meta))| agent_line(info, *meta, Some(i) == app.selected_agent, ascii, app))
        .collect();
    let para = Paragraph::new(lines).alignment(Alignment::Left);
    para.render(inner, buf);
}

/// Render the indent + branch glyph for a tree row at the given
/// depth. Depth 0 produces a single leading space — same as today's
/// flat-list shape. Depth 1 produces ` ├─ ` / ` └─ ` (or ` |- ` /
/// `` `- `` in ASCII), with a matching leading space so the branch
/// glyph sits one indent past the depth-0 leading column (owner
/// alignment review, tg msg 1892). Depth >= 2 isn't a v1 goal per
/// the issue's non-goal list; the renderer clamps to the depth-1
/// shape behind a single extra indent so a chained schema (if one
/// ever slips past validation) at least lays out top-to-bottom
/// without crashing.
fn tree_prefix(meta: TreeRowMeta, ascii: bool) -> String {
    let branch = match (meta.is_last_sibling, ascii) {
        (false, false) => "├─",
        (true, false) => "└─",
        (false, true) => "|-",
        (true, true) => "`-",
    };
    match meta.depth {
        0 => " ".to_string(),
        1 => format!(" {branch} "),
        // Defensive clamp for any chain past v1's schema scope.
        _ => format!("   {branch} "),
    }
}

fn agent_line<'a>(
    info: &'a AgentInfo,
    meta: TreeRowMeta,
    selected: bool,
    ascii: bool,
    app: &App,
) -> Line<'a> {
    let glyph = state_glyph(info, ascii);
    // T-160: roster prefers the operator's display_name when set,
    // falling back to the YAML key (`info.agent`) — the canonical id
    // would over-prefix the project and is reserved for cross-project
    // surfaces like the detail header and wall-tile title.
    let label = info.display_name.as_deref().unwrap_or(&info.agent);
    let prefix = tree_prefix(meta, ascii);
    let display = format!("{prefix}{glyph}  {label}");
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
    // T-108: when stream-mode is active, mark the border title with a
    // bright `[STREAM-KEYS]` tag so the pane the operator's typing
    // into is unambiguous even at a glance away from the statusline.
    let stream = matches!(app.stage, crate::app::Stage::StreamKeys);
    let title = match app
        .selected_agent
        .and_then(|i| app.team.agents.get(i))
        .map(|a| crate::data::agent_label(&app.team, &a.id))
    {
        Some(label) if stream => format!("DETAIL · {label}  [STREAM-KEYS]"),
        Some(label) => format!("DETAIL · {label}"),
        None if stream => "DETAIL  [STREAM-KEYS]".to_string(),
        None => "DETAIL".to_string(),
    };
    // While stream-mode is active, force the focus-ring style on the
    // detail pane regardless of `focused_pane`. Pane focus and stream-
    // mode are aligned in practice (entry is gated on detail focus),
    // but a future refactor that lets focus drift mid-mode shouldn't
    // visually downgrade the active stream pane.
    let outer_block = pane_block(&title, focused_pane || stream, app);
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
    let label = crate::data::agent_label(&app.team, agent_id);
    let title = format!(" {glyph} {label} ");
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

    // T-131 PR-2: between tabs and body, conditionally reserve one
    // line for either the open filter / search input bar OR the
    // always-visible filter-state indicator when filter / search are
    // non-empty but the input is closed. Neither condition active →
    // height 0, body gets all the space (matches PR-1 layout).
    let tab = app.mailbox_tab;
    let input_open = app.mailbox_input_mode.is_some();
    let filter = app.mailbox.filter_text(tab);
    let search = app.mailbox.search_text(tab);
    let indicator_visible = !input_open && (!filter.is_empty() || !search.is_empty());
    let aux_height = if input_open || indicator_visible {
        1
    } else {
        0
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // tabs
            Constraint::Length(aux_height), // input bar OR state indicator (0 when neither)
            Constraint::Min(0),             // body
        ])
        .split(inner);

    render_mailbox_tabs(buf, layout[0], app);
    if aux_height == 1 {
        render_mailbox_aux(buf, layout[1], app);
    }
    render_mailbox_body(buf, layout[2], app);
}

fn render_mailbox_aux(buf: &mut Buffer, area: Rect, app: &App) {
    // T-131 PR-2: one-line auxiliary row between mailbox tabs and
    // body. Either:
    //   - the active input bar (`filter: foo█` / `search: bar█`,
    //     with a faux cursor block at the end of the typed text),
    //     when an input is open; or
    //   - the always-visible filter-state indicator (`filter: foo
    //     search: bar`) when input is closed but at least one axis
    //     is non-empty — without this, closing the input leaves a
    //     shorter row list with no signal why rows disappeared,
    //     the UX bug the issue flagged.
    let tab = app.mailbox_tab;
    let muted = Style::default().fg(app.capabilities.muted());
    let text = match app.mailbox_input_mode {
        Some(MailboxInputKind::Filter) => {
            format!("filter: {}\u{2588}", app.mailbox.filter_text(tab))
        }
        Some(MailboxInputKind::Search) => {
            format!("search: {}\u{2588}", app.mailbox.search_text(tab))
        }
        None => {
            // Closed input but at least one axis non-empty (caller
            // gated; this branch is the indicator only).
            let filter = app.mailbox.filter_text(tab);
            let search = app.mailbox.search_text(tab);
            match (filter.is_empty(), search.is_empty()) {
                (false, false) => format!("filter: {filter}  search: {search}"),
                (false, true) => format!("filter: {filter}"),
                (true, false) => format!("search: {search}"),
                (true, true) => String::new(), // unreachable per gate
            }
        }
    };
    Paragraph::new(text).style(muted).render(area, buf);
}

fn render_mailbox_tabs(buf: &mut Buffer, area: Rect, app: &App) {
    // `Inbox  Sent  Channel  Wire` — active tab gets the focus
    // accent (REVERSED so it reads as a highlight bar even in
    // monochrome terminals where colour alone wouldn't carry the
    // signal).
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

    // T-131 PR-1: cursor-aware window + selected-row highlight.
    // `visible_indices` is identity in PR-1 (every row visible); PR-2
    // (filter+search) swaps its body — the rest of this render path
    // is the abstraction's call site and does not need changing.
    let visible = app.mailbox.visible_indices(app.mailbox_tab);
    if visible.is_empty() {
        // PR-1: unreachable (rows non-empty implies visible non-empty),
        // but stays here as the PR-2 empty-filter-result branch.
        return;
    }
    let cap = area.height as usize;
    let selected = app
        .mailbox
        .cursor(app.mailbox_tab)
        .selected_idx
        .min(visible.len() - 1);
    // Tail-anchored: when selected sits in the last `cap` rows, anchor
    // to the tail so the freshly-appended row stays visible — matches
    // the pre-T-131 default. Otherwise keep selected near the bottom
    // of the window (vim-like). On terminals taller than the row count
    // the whole list fits and `start` is 0.
    let start = if visible.len() <= cap {
        0
    } else if visible.len() - selected <= cap {
        visible.len() - cap
    } else {
        selected.saturating_sub(cap.saturating_sub(1))
    };
    let end = (start + cap).min(visible.len());
    let focused = app.focused_pane == Pane::Mailbox;
    let highlight = Style::default().add_modifier(Modifier::REVERSED);
    // T-231: pass the active tab so render_row can pick the right
    // prefix (sender for Inbox/Channel/Wire, recipient for Sent).
    let lines: Vec<Line<'_>> = visible[start..end]
        .iter()
        .map(|&row_idx| {
            let line = Line::raw(render_row(&rows[row_idx], &app.team, app.mailbox_tab));
            if focused && row_idx == visible[selected] {
                line.style(highlight)
            } else {
                line
            }
        })
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
