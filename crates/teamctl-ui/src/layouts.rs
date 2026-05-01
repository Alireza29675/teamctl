//! Alternate main-view layouts (PR-UI-6).
//!
//! - `Wall` — orchestrator overview. Up to 4 agent tiles in a 2×2
//!   grid (or 1×N stack on narrow terminals). >4 agents scroll
//!   the grid vertically per Alireza's v2-locked answer.
//! - `MailboxFirst` — channel-list / feed / participants
//!   horizontal split, for triaging mailbox traffic across the team
//!   when the operator's focus is communication-first rather than
//!   one-agent-deep.
//!
//! Both layouts share the same statusline below them; both are
//! reachable from the Triptych layout via `Ctrl+W` / `Ctrl+M`.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout as RtLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::app::App;
use crate::data::{state_glyph, AgentInfo};
use crate::theme::ColorMode;

/// 4 visible tiles + vertical scroll for >4 agents — pin matches
/// SPEC §3 / Alireza v2-locked answer.
pub const WALL_TILE_CAP: usize = 4;

pub struct Wall<'a> {
    pub app: &'a App,
}

impl Widget for Wall<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let agents = &self.app.team.agents;
        if agents.is_empty() {
            Paragraph::new("(no agents)")
                .style(Style::default().fg(self.app.capabilities.muted()))
                .alignment(Alignment::Center)
                .render(area, buf);
            return;
        }

        // Scroll window: starting from `wall_scroll`, take up to
        // `WALL_TILE_CAP` agents. Operator scrolls with `J`/`K` /
        // PageUp/PageDown via App::wall_scroll_*.
        let start = self.app.wall_scroll.min(agents.len().saturating_sub(1));
        let end = (start + WALL_TILE_CAP).min(agents.len());
        let window: Vec<&AgentInfo> = agents[start..end].iter().collect();

        // 2×2 grid: split the area into 2 rows, each row into 2
        // cols. Narrow terminals (height < 12) collapse to a 1×N
        // vertical stack so each tile keeps a readable footprint.
        let stack_vertically = area.height < 12;
        let ascii = matches!(self.app.capabilities.color, ColorMode::Monochrome);

        if stack_vertically {
            let rows = RtLayout::default()
                .direction(Direction::Vertical)
                .constraints(vec![
                    Constraint::Ratio(1, window.len().max(1) as u32);
                    window.len().max(1)
                ])
                .split(area);
            for (i, info) in window.iter().enumerate() {
                let selected = (start + i) == self.app.selected_agent.unwrap_or(usize::MAX);
                render_tile(buf, rows[i], info, selected, ascii, self.app);
            }
            return;
        }

        let rows = RtLayout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        for (row_idx, row_area) in rows.iter().enumerate() {
            let cells = RtLayout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(*row_area);
            for (col_idx, cell_area) in cells.iter().enumerate() {
                let tile_idx = row_idx * 2 + col_idx;
                if tile_idx < window.len() {
                    let info = window[tile_idx];
                    let selected =
                        (start + tile_idx) == self.app.selected_agent.unwrap_or(usize::MAX);
                    render_tile(buf, *cell_area, info, selected, ascii, self.app);
                }
            }
        }
    }
}

fn render_tile(
    buf: &mut Buffer,
    area: Rect,
    info: &AgentInfo,
    selected: bool,
    ascii: bool,
    app: &App,
) {
    let glyph = state_glyph(info, ascii);
    let title = format!(" {glyph} {} ", info.id);
    let border_style = if selected {
        Style::default()
            .fg(app.capabilities.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.capabilities.muted())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    block.render(area, buf);

    // Last 4 lines from the focused-agent's detail buffer when
    // this tile is the focused agent; otherwise an empty hint
    // (real per-tile pane captures are not in PR-UI-6 scope —
    // SPEC explicitly defers to a future cycle).
    let lines: Vec<Line<'_>> = if selected && !app.detail_buffer.is_empty() {
        let cap = (inner.height as usize).min(4);
        let start = app.detail_buffer.len().saturating_sub(cap);
        app.detail_buffer[start..]
            .iter()
            .map(|s| Line::raw(s.clone()))
            .collect()
    } else {
        vec![Line::styled(
            "(focus this tile to stream)",
            Style::default().fg(app.capabilities.muted()),
        )]
    };
    Paragraph::new(lines).render(inner, buf);
}

/// `MailboxFirst` — channel-list (left, ~26 cols) / feed (middle,
/// flex) / participants (right, ~24 cols).
pub struct MailboxFirst<'a> {
    pub app: &'a App,
}

impl Widget for MailboxFirst<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let columns = RtLayout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(26),
                Constraint::Min(0),
                Constraint::Length(24),
            ])
            .split(area);
        render_channels_list(buf, columns[0], self.app);
        render_channel_feed(buf, columns[1], self.app);
        render_participants(buf, columns[2], self.app);
    }
}

fn render_channels_list(buf: &mut Buffer, area: Rect, app: &App) {
    let block = Block::default()
        .title("CHANNELS")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.muted()));
    let inner = block.inner(area);
    block.render(area, buf);
    if app.team.channels.is_empty() {
        Paragraph::new("(no channels)")
            .style(Style::default().fg(app.capabilities.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    let lines: Vec<Line<'_>> = app
        .team
        .channels
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let label = format!("  #{}", ch.name);
            let style = if Some(i) == app.selected_channel {
                Style::default()
                    .fg(app.capabilities.accent())
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::styled(label, style)
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

fn render_channel_feed(buf: &mut Buffer, area: Rect, app: &App) {
    let selected = app.selected_channel.and_then(|i| app.team.channels.get(i));
    let title = match selected {
        Some(ch) => format!("FEED · #{}", ch.name),
        None => "FEED".into(),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.muted()));
    let inner = block.inner(area);
    block.render(area, buf);
    // PR-UI-6 fixup (Q3, dev2 review): the rolled-up
    // `mailbox.channel` buffer carries every channel row the
    // focused agent receives; filter to the selected channel so
    // the title's `FEED · #editorial` reads truthfully. Rows
    // whose `recipient` doesn't match `channel:<channel.id>` get
    // dropped on the floor.
    let all_rows = app.mailbox.rows(crate::mailbox::MailboxTab::Channel);
    let filtered: Vec<&crate::mailbox::MessageRow> = match selected {
        Some(ch) => filter_rows_for_channel(all_rows, &ch.id),
        None => all_rows.iter().collect(),
    };
    if filtered.is_empty() {
        Paragraph::new("(no channel traffic)")
            .style(Style::default().fg(app.capabilities.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    let cap = inner.height as usize;
    let start = filtered.len().saturating_sub(cap);
    let lines: Vec<Line<'_>> = filtered[start..]
        .iter()
        .map(|r| Line::raw(crate::mailbox::render_row(r)))
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

fn render_participants(buf: &mut Buffer, area: Rect, app: &App) {
    let block = Block::default()
        .title("PARTICIPANTS")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.muted()));
    let inner = block.inner(area);
    block.render(area, buf);
    // PR-UI-6 derives "participants" as every agent in the
    // focused channel's project — a serviceable approximation of
    // membership without a dedicated query. PR-UI-7's polish cycle
    // can wire `channel_members` table proper.
    let project = app
        .selected_channel
        .and_then(|i| app.team.channels.get(i))
        .map(|c| c.project_id.clone());
    let participants: Vec<&AgentInfo> = match project {
        Some(p) => app.team.agents.iter().filter(|a| a.project == p).collect(),
        None => Vec::new(),
    };
    if participants.is_empty() {
        Paragraph::new("(none)")
            .style(Style::default().fg(app.capabilities.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    let lines: Vec<Line<'_>> = participants
        .iter()
        .map(|info| Line::raw(format!("  {}", info.agent)))
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

/// Drop every row whose `recipient` doesn't match
/// `channel:<channel_id>`. Pulled out as a free function so unit
/// tests can pin the contract without rendering — feed pane Q3
/// fixup per dev2's PR-UI-6 review.
pub fn filter_rows_for_channel<'a>(
    rows: &'a [crate::mailbox::MessageRow],
    channel_id: &str,
) -> Vec<&'a crate::mailbox::MessageRow> {
    let target = format!("channel:{channel_id}");
    rows.iter().filter(|r| r.recipient == target).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::MessageRow;

    fn row(id: i64, recipient: &str) -> MessageRow {
        MessageRow {
            id,
            sender: "p:m".into(),
            recipient: recipient.into(),
            text: format!("body {id}"),
            sent_at: 0.0,
        }
    }

    #[test]
    fn filter_keeps_only_matching_channel_rows() {
        let rows = vec![
            row(1, "channel:writing:editorial"),
            row(2, "channel:writing:critique"),
            row(3, "channel:writing:editorial"),
            row(4, "channel:writing:all"),
        ];
        let kept = filter_rows_for_channel(&rows, "writing:editorial");
        let ids: Vec<i64> = kept.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn filter_returns_empty_when_no_rows_match() {
        let rows = vec![
            row(1, "channel:writing:critique"),
            row(2, "channel:writing:all"),
        ];
        let kept = filter_rows_for_channel(&rows, "writing:editorial");
        assert!(kept.is_empty());
    }

    #[test]
    fn filter_does_not_match_dm_rows_with_same_id_suffix() {
        // A DM to `<project>:<agent>` must never leak into a
        // channel-feed view, even when the agent name happens to
        // collide with a channel name. The `channel:` prefix in
        // the target string keeps that disjoint.
        let rows = vec![
            row(1, "writing:editorial"), // looks like an agent id
            row(2, "channel:writing:editorial"),
        ];
        let kept = filter_rows_for_channel(&rows, "writing:editorial");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, 2);
    }
}
