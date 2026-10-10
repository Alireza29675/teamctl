//! Rendering shared by the terminal and deterministic buffers.

use super::{App, Stage};
use crate::{layouts, splash, status_bar, statusline, triptych};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    render(app, area, f.buffer_mut());
}

fn render_help_overlay(area: Rect, buf: &mut Buffer, app: &App) {
    let popup_w = 70u16.min(area.width.saturating_sub(4));
    let popup_h = 24u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    let block = Block::default()
        .title("help · ? to close")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.accent()));
    let inner = block.inner(popup);
    block.render(popup, buf);
    let muted = Style::default().fg(app.capabilities.muted());
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<ratatui::text::Line<'_>> = Vec::new();
    for group in crate::help::ALL_GROUPS {
        lines.push(ratatui::text::Line::styled(group.title, bold));
        for b in group.bindings {
            lines.push(ratatui::text::Line::raw(format!(
                "  {:<22}  {}",
                b.chord, b.description
            )));
        }
        lines.push(ratatui::text::Line::styled("", muted));
    }
    Paragraph::new(lines).render(inner, buf);
}

/// T-131 PR-3: detail modal — full message body (wrapped + scrollable)
/// plus sender / recipient / kind / absolute timestamp / transport /
/// message id. Renders only when `app.mailbox_detail_modal` is `Some`;
/// otherwise a silent no-op so a render race during close-tearing
/// can't crash. The snapshot is the source of truth for everything
/// the modal shows — the underlying buffer is NOT consulted, which
/// is the variant-(a) snapshot-at-open contract.
fn render_mailbox_detail_modal(area: Rect, buf: &mut Buffer, app: &App) {
    let Some(row) = app.mailbox_detail_modal.as_ref() else {
        return;
    };
    let popup_w = 80u16.min(area.width.saturating_sub(4));
    let popup_h = 24u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    let title = format!("MESSAGE · id {} · Esc/q to close", row.id);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.accent()));
    let inner = block.inner(popup);
    block.render(popup, buf);
    if inner.height == 0 {
        return;
    }

    // Metadata header (5 lines) + 1 blank separator + body fills the
    // rest. Fixed metadata height keeps the body scroll math simple:
    // body height = inner.height - 6, and Paragraph::scroll((n, 0))
    // hides the first n wrapped lines.
    const META_LINES: u16 = 6;
    let meta_h = META_LINES.min(inner.height);
    let body_h = inner.height.saturating_sub(meta_h);
    let meta_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: meta_h,
    };
    let body_area = Rect {
        x: inner.x,
        y: inner.y + meta_h,
        width: inner.width,
        height: body_h,
    };

    // Format the absolute timestamp in UTC with timezone (matches
    // issue's "absolute, with timezone"). UTC is unambiguous across
    // operator timezones — the local-time variant would need extra
    // care for the dogfood team's mixed-locale operators.
    let ts = chrono::DateTime::<chrono::Utc>::from_timestamp(
        row.sent_at as i64,
        ((row.sent_at.fract() * 1_000_000_000.0) as u32).min(999_999_999),
    )
    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
    .unwrap_or_else(|| "—".to_string());

    let muted = Style::default().fg(app.capabilities.muted());
    let meta_lines = vec![
        ratatui::text::Line::raw(format!("from:      {}", row.sender)),
        ratatui::text::Line::raw(format!("to:        {}", row.recipient)),
        ratatui::text::Line::raw(format!("kind:      {}", crate::mailbox::kind_label(row))),
        ratatui::text::Line::raw(format!("time:      {ts}")),
        ratatui::text::Line::raw(format!(
            "transport: {}",
            crate::mailbox::transport_label(row)
        )),
        ratatui::text::Line::styled("", muted),
    ];
    Paragraph::new(meta_lines)
        .style(Style::default())
        .render(meta_area, buf);

    // Body: wrap to inner width, scroll by the operator's offset.
    // No upper-bound clamp on scroll here — Paragraph's scroll
    // semantics tolerate values past the end (renders empty), and
    // the j/k handlers' saturating_add caps at u16::MAX which is
    // far beyond any realistic body length.
    Paragraph::new(row.text.clone())
        .wrap(Wrap { trim: false })
        .scroll((app.mailbox_detail_scroll, 0))
        .render(body_area, buf);
}

fn render_tutorial(area: Rect, buf: &mut Buffer, app: &App) {
    let popup_w = 64u16.min(area.width.saturating_sub(4));
    let popup_h = 14u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    let total = crate::onboarding::STEPS.len();
    let i = app.tutorial_step.min(total.saturating_sub(1));
    let step = &crate::onboarding::STEPS[i];
    let block = Block::default()
        .title(format!("tutorial · {}/{total}", i + 1))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.accent()));
    let inner = block.inner(popup);
    block.render(popup, buf);
    let muted = Style::default().fg(app.capabilities.muted());
    let lines = vec![
        ratatui::text::Line::styled(step.heading, Style::default().add_modifier(Modifier::BOLD)),
        ratatui::text::Line::raw(""),
        ratatui::text::Line::raw(step.body),
        ratatui::text::Line::raw(""),
        ratatui::text::Line::styled("any key next  ·  k / ↑ / p back  ·  Esc skip", muted),
    ];
    // T-074 bug 5: tutorial bodies are prose paragraphs, not pre-
    // formatted lines — clip-on-overflow leaves them looking truncated
    // on common (≤80 col) terminals. Soft-wrap with `trim: true` so
    // long step descriptions reflow into the modal width instead of
    // dropping off the right edge.
    Paragraph::new(lines)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .render(inner, buf);
}

fn render_compose_picker_body(inner: Rect, buf: &mut Buffer, app: &App) {
    let muted = Style::default().fg(app.capabilities.muted());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let lines: Vec<ratatui::text::Line<'_>> = if app.team.channels.is_empty() {
        vec![ratatui::text::Line::styled(
            "(no channels declared in team-compose)",
            muted,
        )]
    } else {
        app.team
            .channels
            .iter()
            .enumerate()
            .map(|(i, ch)| {
                let label = format!("  #{}  ({})", ch.name, ch.project_id);
                let style = if i == app.compose_picker_index {
                    Style::default()
                        .fg(app.capabilities.accent())
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ratatui::text::Line::styled(label, style)
            })
            .collect()
    };
    let top = app
        .compose_picker_index
        .saturating_sub(chunks[0].height.saturating_sub(1) as usize);
    Paragraph::new(lines)
        .scroll((top.min(u16::MAX as usize) as u16, 0))
        .render(chunks[0], buf);
    Paragraph::new("pick a channel to broadcast to")
        .style(muted)
        .render(chunks[1], buf);
    Paragraph::new("Enter pick · j/k navigate · Esc cancel")
        .style(muted)
        .render(chunks[2], buf);
}

fn render_compose_modal(area: Rect, buf: &mut Buffer, app: &App) {
    let popup_w = 80u16.min(area.width.saturating_sub(4));
    let popup_h = 16u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    let title = app
        .compose_target
        .as_ref()
        .map(|t| t.title(&app.team))
        .unwrap_or_else(|| "→ ?".into());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.accent()));
    let inner = block.inner(popup);
    block.render(popup, buf);

    if inner.height < 3 {
        return;
    }
    // PR-UI-6: when the broadcast picker is open we render a
    // channel-list inside the modal instead of the editor; the
    // editor footer stays so operators see the same layout.
    if app.compose_picker_open {
        render_compose_picker_body(inner, buf, app);
        return;
    }
    if app.compose_attach_input_open {
        render_compose_attach_input(inner, buf, app);
        return;
    }
    // Reserve the bottom two rows: an error line (rendered when
    // present, blank otherwise) and the footer with key hints.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // editor body
            Constraint::Length(1), // error / status
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Body — render lines with a `▏` cursor marker on the active
    // row when in Insert. Skip cursor cell in Normal/Ex modes so
    // the operator's eye finds the row by row context, not a
    // blinking caret.
    let muted = Style::default().fg(app.capabilities.muted());
    let body_lines: Vec<ratatui::text::Line<'_>> = app
        .compose_editor
        .lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            if row == app.compose_editor.cursor_row
                && app.compose_editor.mode == crate::compose::VimMode::Insert
            {
                let col = app.compose_editor.cursor_byte_col();
                let (head, tail) = line.split_at(col);
                ratatui::text::Line::from(vec![
                    ratatui::text::Span::raw(head.to_string()),
                    ratatui::text::Span::styled(
                        "▏",
                        Style::default().fg(app.capabilities.accent()),
                    ),
                    ratatui::text::Span::raw(tail.to_string()),
                ])
            } else {
                ratatui::text::Line::raw(line.clone())
            }
        })
        .collect();
    Paragraph::new(body_lines)
        .scroll(
            app.compose_editor
                .viewport(chunks[0].width, chunks[0].height),
        )
        .render(chunks[0], buf);

    let error_line = match (&app.compose_error, app.compose_editor.mode) {
        (Some(e), _) => format!("error: {e}"),
        (None, crate::compose::VimMode::Ex) => format!(":{}", app.compose_editor.ex_buffer),
        (None, crate::compose::VimMode::Normal) => "-- NORMAL --".into(),
        (None, crate::compose::VimMode::Insert) => "-- INSERT --".into(),
    };
    let style = if app.compose_error.is_some() {
        Style::default().fg(app.capabilities.accent())
    } else {
        muted
    };
    Paragraph::new(error_line)
        .style(style)
        .render(chunks[1], buf);

    Paragraph::new("Esc Enter send · Esc Esc cancel · Tab attach")
        .style(muted)
        .render(chunks[2], buf);
}

/// T-32: render the path-input overlay. Single-line buffer + a
/// caret marker, with hints for confirm/cancel. Mirrors the picker
/// overlay's layout (body / status line / footer) so the modal's
/// vertical rhythm doesn't shift between the two overlays.
fn render_compose_attach_input(inner: Rect, buf: &mut Buffer, app: &App) {
    let muted = Style::default().fg(app.capabilities.muted());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let line = ratatui::text::Line::from(vec![
        ratatui::text::Span::raw(format!("path: {}", app.compose_attach_buffer)),
        ratatui::text::Span::styled("▏", Style::default().fg(app.capabilities.accent())),
    ]);
    Paragraph::new(line).render(chunks[0], buf);
    Paragraph::new("type or paste an absolute path; the agent reads it via the broker")
        .style(muted)
        .render(chunks[1], buf);
    Paragraph::new("Enter confirm · Esc cancel")
        .style(muted)
        .render(chunks[2], buf);
}

fn render_approvals_modal(area: Rect, buf: &mut Buffer, app: &App) {
    let popup_w = 80u16.min(area.width.saturating_sub(4));
    let popup_h = 18u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(popup_w, popup_h, area);
    Clear.render(popup, buf);
    let n = app.pending_approvals.len();
    let i = app.selected_approval.min(n.saturating_sub(1));
    let title = format!("approvals · {}/{n}", i + 1);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.capabilities.accent()));
    let inner = block.inner(popup);
    block.render(popup, buf);

    let muted = Style::default().fg(app.capabilities.muted());
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let Some(a) = app.focused_approval() else {
        Paragraph::new("(no pending approvals)")
            .style(muted)
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    };

    let mut lines: Vec<ratatui::text::Line<'_>> = vec![
        ratatui::text::Line::styled(format!("#{}  {}", a.id, a.action), bold),
        ratatui::text::Line::styled(
            format!("from: {}", crate::data::agent_label(&app.team, &a.agent_id)),
            muted,
        ),
        ratatui::text::Line::raw(""),
        ratatui::text::Line::raw(a.summary.clone()),
    ];
    if !a.payload_json.is_empty() && a.payload_json != "{}" {
        lines.push(ratatui::text::Line::raw(""));
        lines.push(ratatui::text::Line::styled("payload:", muted));
        for chunk in a.payload_json.lines().take(4) {
            lines.push(ratatui::text::Line::raw(chunk.to_string()));
        }
    }
    if let Some(err) = &app.approval_error {
        lines.push(ratatui::text::Line::raw(""));
        lines.push(ratatui::text::Line::styled(
            format!("error: {err}"),
            Style::default().fg(app.capabilities.accent()),
        ));
    }
    lines.push(ratatui::text::Line::raw(""));
    lines.push(ratatui::text::Line::styled(
        "[y] approve  ·  [Shift-N] deny  ·  [j/k] cycle  ·  [Esc] close",
        muted,
    ));
    Paragraph::new(lines).render(inner, buf);
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

/// `Terminal`. It uses the same renderer as the interactive path.
pub fn render_to_buffer(app: &App, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render(app, area, &mut buf);
    buf
}

fn render(app: &App, area: Rect, buf: &mut Buffer) {
    match app.stage {
        Stage::Splash => splash::Splash { app }.render(area, buf),
        Stage::Triptych => render_main(app, area, buf),
        Stage::StreamKeys => render_main(app, area, buf),
        Stage::QuitConfirm => {
            render_main(app, area, buf);
            render_quit_confirm(area, buf);
        }
        Stage::ApprovalsModal => {
            render_main(app, area, buf);
            render_approvals_modal(area, buf, app);
        }
        Stage::ComposeModal => {
            render_main(app, area, buf);
            render_compose_modal(area, buf, app);
        }
        Stage::HelpOverlay => {
            render_main(app, area, buf);
            render_help_overlay(area, buf, app);
        }
        Stage::Tutorial => {
            render_main(app, area, buf);
            render_tutorial(area, buf, app);
        }
        Stage::MailboxDetailModal => {
            render_main(app, area, buf);
            render_mailbox_detail_modal(area, buf, app);
        }
    }
}

fn render_main(app: &App, area: Rect, buf: &mut Buffer) {
    let chunks = crate::pane_resize::main_areas(area);
    match app.layout {
        crate::triptych::MainLayout::Triptych => {
            triptych::Triptych { app }.render(chunks[0], buf);
        }
        crate::triptych::MainLayout::Wall => {
            layouts::Wall { app }.render(chunks[0], buf);
        }
        crate::triptych::MainLayout::MailboxFirst => {
            layouts::MailboxFirst { app }.render(chunks[0], buf);
        }
    }
    statusline::Statusline { app }.render(chunks[1], buf);
    status_bar::StatusBar { app }.render(chunks[2], buf);
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
