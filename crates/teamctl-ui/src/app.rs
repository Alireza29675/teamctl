//! App state and the top-level run loop.
//!
//! Three stages today: `Splash` (figlet logo for ~3s or until first
//! key), `Triptych` (the default read view, now backed by a live
//! team snapshot from PR-UI-2), and `QuitConfirm` (a modal asking
//! "really?"). Subsequent stacked PRs bolt on more modals and the
//! layout variants from SPEC §3 — those wire in by adding `Stage`
//! variants and dispatching from `draw`/`handle_event`, no
//! rearchitecting.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use crate::approvals::{
    Approval, ApprovalDecider, ApprovalSource, BrokerApprovalSource, CliApprovalDecider, Decision,
};
use crate::data::TeamSnapshot;
use crate::mailbox::{BrokerMailboxSource, MailboxBuffers, MailboxSource, MailboxTab};
use crate::pane::{PaneSource, TmuxPaneSource};
use crate::splash;
use crate::statusline;
use crate::theme::{detect_capabilities, Capabilities};
use crate::triptych::{self, Pane};
use crate::tutorial;
use crate::watch::Watch;

const SPLASH_AUTO_DISMISS: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How often the team snapshot + detail-pane capture get refreshed.
/// PR-UI-2 polls; PR-UI-3 may upgrade to event subscriptions.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Splash,
    Triptych,
    QuitConfirm,
    /// Approvals modal — opens on `a` (only when there's a
    /// pending approval), routes Approve/Deny via the existing
    /// `teamctl approve|deny` CLI so T-031's `delivered_at`
    /// contract stays honored.
    ApprovalsModal,
}

pub struct App {
    pub stage: Stage,
    /// Tracked so QuitConfirm can return to whichever stage opened it.
    pub previous_stage: Stage,
    pub focused_pane: Pane,
    pub team: TeamSnapshot,
    /// Index into `team.agents` of the agent the detail pane is
    /// streaming. `None` when the team is empty or roster
    /// navigation hasn't picked one yet.
    pub selected_agent: Option<usize>,
    /// Lines from the most recent pane capture. Bounded to the last
    /// `MAX_DETAIL_LINES` so the buffer doesn't grow unboundedly
    /// over a long-running session.
    pub detail_buffer: Vec<String>,
    pub version: &'static str,
    pub capabilities: Capabilities,
    pub splash_started: Instant,
    /// Last time the snapshot + pane capture were refreshed. Used by
    /// `tick()` to gate the next refresh.
    pub last_refresh: Instant,
    pub running: bool,
    /// First-launch detection — when the marker file exists, future
    /// stacked-PRs (PR-UI-7) skip the tutorial after splash. PR-UI-1
    /// only reads the flag; nothing routes off it yet.
    pub tutorial_completed: bool,
    /// Active tab inside the mailbox pane (PR-UI-3). `Tab` cycles
    /// these when `focused_pane == Mailbox`; otherwise `Tab` cycles
    /// the panes themselves (PR-UI-1 behaviour).
    pub mailbox_tab: MailboxTab,
    /// Per-tab buffers + cursors for the focused agent's mailbox
    /// view. Reset whenever the focused agent changes — switching
    /// agents starts the operator at the head of fresh traffic.
    pub mailbox: MailboxBuffers,
    /// Pending approvals snapshot (PR-UI-4). Drives the conditional
    /// stripe at the top of Triptych and the modal opened by `a`.
    pub pending_approvals: Vec<Approval>,
    /// Index into `pending_approvals` of the row the modal is
    /// currently showing. Reset to 0 each time the modal opens;
    /// `j` / `k` (or `↑` / `↓`) cycle.
    pub selected_approval: usize,
    /// Last error from a CLI-routed Approve/Deny call — surfaced
    /// inline in the modal so the operator sees why a decision
    /// didn't take.
    pub approval_error: Option<String>,
}

const MAX_DETAIL_LINES: usize = 2000;

impl App {
    /// Construct an empty App — no team snapshot loaded. Used by
    /// tests and as the splash-stage default. Production launch
    /// goes through `App::launch()` which immediately runs an
    /// initial `refresh()` so the splash screen already shows the
    /// real team name + agent count.
    pub fn new() -> Self {
        Self {
            stage: Stage::Splash,
            previous_stage: Stage::Splash,
            focused_pane: Pane::Roster,
            team: TeamSnapshot::empty(std::path::PathBuf::new()),
            selected_agent: None,
            detail_buffer: Vec::new(),
            version: env!("CARGO_PKG_VERSION"),
            capabilities: detect_capabilities(),
            splash_started: Instant::now(),
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            running: true,
            tutorial_completed: tutorial::is_completed(),
            mailbox_tab: MailboxTab::Inbox,
            mailbox: MailboxBuffers::default(),
            pending_approvals: Vec::new(),
            selected_approval: 0,
            approval_error: None,
        }
    }

    pub fn cycle_mailbox_tab(&mut self) {
        self.mailbox_tab = self.mailbox_tab.next();
    }

    pub fn cycle_focus_back(&mut self) {
        self.focused_pane = self.focused_pane.prev();
    }

    pub fn has_pending_approvals(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    pub fn enter_approvals_modal(&mut self) {
        if self.pending_approvals.is_empty() {
            return;
        }
        self.previous_stage = self.stage;
        self.stage = Stage::ApprovalsModal;
        self.selected_approval = 0;
        self.approval_error = None;
    }

    pub fn close_approvals_modal(&mut self) {
        self.stage = self.previous_stage;
        self.approval_error = None;
    }

    pub fn cycle_approval_next(&mut self) {
        if self.pending_approvals.is_empty() {
            return;
        }
        self.selected_approval = (self.selected_approval + 1) % self.pending_approvals.len();
    }

    pub fn cycle_approval_prev(&mut self) {
        if self.pending_approvals.is_empty() {
            return;
        }
        self.selected_approval = if self.selected_approval == 0 {
            self.pending_approvals.len() - 1
        } else {
            self.selected_approval - 1
        };
    }

    pub fn focused_approval(&self) -> Option<&Approval> {
        self.pending_approvals.get(self.selected_approval)
    }

    /// Replace the pending-approvals list. Closes the modal when
    /// the queue empties (no row to act on); preserves the modal
    /// otherwise but clamps `selected_approval` into range so an
    /// approval resolved out-of-band doesn't leave us pointing at
    /// a stale index.
    pub fn replace_approvals(&mut self, approvals: Vec<Approval>) {
        self.pending_approvals = approvals;
        if self.pending_approvals.is_empty() {
            if matches!(self.stage, Stage::ApprovalsModal) {
                self.close_approvals_modal();
            }
            self.selected_approval = 0;
        } else if self.selected_approval >= self.pending_approvals.len() {
            self.selected_approval = self.pending_approvals.len() - 1;
        }
    }

    /// Apply a decision to the focused approval via the injected
    /// decider. The decider routes through `teamctl approve|deny`
    /// in production; tests inject a recorder. On success the row
    /// gets removed from the local `pending_approvals` snapshot
    /// optimistically — the next `refresh_approvals` will reconcile
    /// against the broker.
    pub fn apply_decision<D: ApprovalDecider>(&mut self, decider: &D, kind: Decision, note: &str) {
        let Some(approval) = self.focused_approval().cloned() else {
            return;
        };
        match decider.decide(&self.team.root, approval.id, kind, note) {
            Ok(()) => {
                self.pending_approvals.retain(|a| a.id != approval.id);
                self.approval_error = None;
                if self.pending_approvals.is_empty() {
                    self.close_approvals_modal();
                } else if self.selected_approval >= self.pending_approvals.len() {
                    self.selected_approval = self.pending_approvals.len() - 1;
                }
            }
            Err(err) => {
                self.approval_error = Some(err.to_string());
            }
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

    /// Move roster selection up by one — wraps at the top. No-op
    /// when the team is empty. Does not change `focused_pane`.
    /// Resets mailbox buffers when the resulting agent id differs
    /// from the prior selection — switching agents should start the
    /// operator at the head of fresh traffic.
    pub fn select_prev(&mut self) {
        if self.team.agents.is_empty() {
            self.selected_agent = None;
            return;
        }
        let prior = self.selected_agent_id();
        self.selected_agent = Some(match self.selected_agent {
            None | Some(0) => self.team.agents.len() - 1,
            Some(i) => i - 1,
        });
        if prior != self.selected_agent_id() {
            self.mailbox.reset();
        }
    }

    /// Move roster selection down by one — wraps at the bottom.
    /// No-op when the team is empty.
    pub fn select_next(&mut self) {
        if self.team.agents.is_empty() {
            self.selected_agent = None;
            return;
        }
        let prior = self.selected_agent_id();
        self.selected_agent = Some(match self.selected_agent {
            None => 0,
            Some(i) => (i + 1) % self.team.agents.len(),
        });
        if prior != self.selected_agent_id() {
            self.mailbox.reset();
        }
    }

    /// `<project>:<agent>` of the currently selected agent, if any.
    pub fn selected_agent_id(&self) -> Option<String> {
        self.selected_agent
            .and_then(|i| self.team.agents.get(i))
            .map(|a| a.id.clone())
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

    /// Replace the team snapshot. Preserves the current selection
    /// when the agent at that index still exists; otherwise resets
    /// to the first agent (or `None` for an empty team). Resets the
    /// mailbox buffers when the resulting agent id differs from the
    /// prior selection — same agent-changed contract as
    /// `select_next` / `select_prev`.
    pub fn replace_team(&mut self, team: TeamSnapshot) {
        let prior_id = self.selected_agent_id();
        self.team = team;
        self.selected_agent = match (prior_id.clone(), self.team.agents.is_empty()) {
            (_, true) => None,
            (Some(id), false) => self.team.agents.iter().position(|a| a.id == id).or(Some(0)),
            (None, false) => Some(0),
        };
        if prior_id != self.selected_agent_id() {
            self.mailbox.reset();
        }
    }

    /// Return the focused agent's tmux session name, if any. Used
    /// by the run loop to know which session to capture.
    pub fn focused_session(&self) -> Option<&str> {
        self.selected_agent
            .and_then(|i| self.team.agents.get(i))
            .map(|a| a.tmux_session.as_str())
    }

    /// Replace the detail buffer, clipped at the recent-line cap.
    pub fn set_detail_buffer(&mut self, lines: Vec<String>) {
        let len = lines.len();
        let start = len.saturating_sub(MAX_DETAIL_LINES);
        self.detail_buffer = lines[start..].to_vec();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Refresh the team snapshot + the focused agent's pane capture +
/// the mailbox tabs (PR-UI-3). Pulled out so tests can drive a
/// single tick deterministically against `MockPaneSource` and
/// `MockMailboxSource` without going through the event loop.
pub fn refresh<P: PaneSource, M: MailboxSource, A: ApprovalSource>(
    app: &mut App,
    pane_source: &P,
    mailbox_source: &M,
    approval_source: &A,
) {
    if let Ok(Some(snapshot)) = TeamSnapshot::discover_and_load() {
        app.replace_team(snapshot);
    }
    if let Some(session) = app.focused_session().map(|s| s.to_string()) {
        if let Ok(lines) = pane_source.capture(&session) {
            app.set_detail_buffer(lines);
        }
    } else {
        app.detail_buffer.clear();
    }
    refresh_mailbox(app, mailbox_source);
    refresh_approvals(app, approval_source);
    app.last_refresh = Instant::now();
}

/// Approvals-only refresh. Extracted on the same shape as
/// `refresh_mailbox` — PR-UI-5+ can call it on its own cadence
/// (e.g. in response to a `notify` signal) without re-running the
/// heavier paths. Errors degrade to "no pending" so the stripe
/// just hides on a transient broker read failure.
pub fn refresh_approvals<A: ApprovalSource>(app: &mut App, approval_source: &A) {
    let approvals = approval_source.pending().unwrap_or_default();
    app.replace_approvals(approvals);
}

/// Mailbox-only refresh — extracted so PR-UI-4+ can call it on its
/// own cadence (e.g. in response to a broker INSERT signal) without
/// re-running the heavier compose + tmux capture path. PR-UI-3
/// just calls it from the main `refresh` once per tick.
pub fn refresh_mailbox<M: MailboxSource>(app: &mut App, mailbox_source: &M) {
    let Some(agent_id) = app.selected_agent_id() else {
        // No agent focused → nothing to fetch. Buffers were already
        // reset on selection change so the empty-state hint shows.
        return;
    };
    let project_id = app
        .selected_agent
        .and_then(|i| app.team.agents.get(i))
        .map(|a| a.project.clone())
        .unwrap_or_default();
    if let Ok(batch) = mailbox_source.inbox(&agent_id, app.mailbox.inbox_after) {
        app.mailbox.extend(MailboxTab::Inbox, batch);
    }
    if let Ok(batch) = mailbox_source.channel_feed(&agent_id, app.mailbox.channel_after) {
        app.mailbox.extend(MailboxTab::Channel, batch);
    }
    if let Ok(batch) = mailbox_source.wire(&project_id, app.mailbox.wire_after) {
        app.mailbox.extend(MailboxTab::Wire, batch);
    }
}

pub fn run<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut app = App::new();
    let pane_source = TmuxPaneSource;
    let decider = CliApprovalDecider;
    // First refresh resolves the team root; only then can we
    // bring up the file-watcher, which keys on `<root>/state/`.
    refresh_with_default_sources(&mut app, &pane_source);
    let mut watch = Watch::try_new(&app.team.root.join("state"));
    while app.running {
        terminal.draw(|f| draw(f, &app))?;
        if event::poll(POLL_INTERVAL)? {
            handle_event(&mut app, event::read()?, &decider);
        }
        if matches!(app.stage, Stage::Splash) && app.splash_started.elapsed() >= SPLASH_AUTO_DISMISS
        {
            app.dismiss_splash();
        }
        // Refresh on either (a) deadline elapsed or (b) the
        // notify-watcher said the broker DB changed. The watcher
        // shaves the typical refresh latency from ~1s to ~50ms when
        // the platform supports it; on platforms without notify
        // support `take_dirty` always returns false and the
        // deadline path is the only trigger (PR-UI-3 behaviour).
        let dirty = watch.take_dirty();
        if dirty || app.last_refresh.elapsed() >= REFRESH_INTERVAL {
            let prior_root = app.team.root.clone();
            refresh_with_default_sources(&mut app, &pane_source);
            // Team root drifted (operator launched in a different
            // tree) → swap the watcher to the new state dir.
            if app.team.root != prior_root {
                watch = Watch::try_new(&app.team.root.join("state"));
            }
        }
    }
    Ok(())
}

/// Build the production `BrokerMailboxSource` + `BrokerApprovalSource`
/// from the current team root and run a refresh with all three
/// default sources. Lives here (rather than inline in `run`) so
/// the team-root → DB-path derivation has one home.
fn refresh_with_default_sources<P: PaneSource>(app: &mut App, pane_source: &P) {
    if let Ok(Some(snapshot)) = TeamSnapshot::discover_and_load() {
        app.replace_team(snapshot);
    }
    let db_path = app.team.root.join("state/mailbox.db");
    let mailbox_source = BrokerMailboxSource::new(db_path.clone());
    let approval_source = BrokerApprovalSource::new(db_path);
    if let Some(session) = app.focused_session().map(|s| s.to_string()) {
        if let Ok(lines) = pane_source.capture(&session) {
            app.set_detail_buffer(lines);
        }
    } else {
        app.detail_buffer.clear();
    }
    refresh_mailbox(app, &mailbox_source);
    refresh_approvals(app, &approval_source);
    app.last_refresh = Instant::now();
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
        Stage::ApprovalsModal => {
            draw_main(f, area, app);
            draw_approvals_modal(f, area, app);
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

fn draw_approvals_modal(f: &mut Frame<'_>, area: Rect, app: &App) {
    let buf = f.buffer_mut();
    render_approvals_modal(area, buf, app);
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
        ratatui::text::Line::styled(format!("from: {}", a.agent_id), muted),
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
        "[Y] approve  ·  [N] deny  ·  [j/k] cycle  ·  [Esc] close",
        muted,
    ));
    Paragraph::new(lines).render(inner, buf);
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

fn handle_event<D: ApprovalDecider>(app: &mut App, ev: Event, decider: &D) {
    use crossterm::event::KeyModifiers;
    match ev {
        Event::Key(k) if k.kind == KeyEventKind::Press => match app.stage {
            Stage::Splash => app.dismiss_splash(),
            Stage::Triptych => match k.code {
                KeyCode::Char('q') => app.enter_quit_confirm(),
                // PR-UI-4: `a` opens the approvals modal when there's
                // at least one pending row. No-op otherwise so the
                // chord doesn't surprise anyone hammering keys.
                KeyCode::Char('a') => app.enter_approvals_modal(),
                // PR-UI-4: Shift+Tab cycles panes backward. Some
                // terminals send `BackTab`, others send `Tab` with
                // SHIFT — handle both.
                KeyCode::BackTab => app.cycle_focus_back(),
                KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => app.cycle_focus_back(),
                // PR-UI-3: when the mailbox pane is focused, `Tab`
                // cycles its three tabs (Inbox / Channel / Wire)
                // rather than the panes — the mailbox is the only
                // pane whose focus state has its own subnavigation,
                // so this special-case stays narrow.
                KeyCode::Tab if app.focused_pane == Pane::Mailbox => app.cycle_mailbox_tab(),
                KeyCode::Tab => app.cycle_focus(),
                // Roster navigation — only when roster is the
                // focused pane. j/k mirror Vim; arrows mirror
                // every-day navigation.
                KeyCode::Up | KeyCode::Char('k') if app.focused_pane == Pane::Roster => {
                    app.select_prev()
                }
                KeyCode::Down | KeyCode::Char('j') if app.focused_pane == Pane::Roster => {
                    app.select_next()
                }
                _ => {}
            },
            Stage::QuitConfirm => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.confirm_quit(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_quit(),
                _ => {}
            },
            Stage::ApprovalsModal => match k.code {
                // Uppercase-only Y / N to commit a decision —
                // requires deliberate Shift, which raises the bar
                // on a destructive deny (and keeps approve on the
                // same chord shape for consistency). Lowercase y/n
                // are intentionally not accepted.
                KeyCode::Char('Y') => app.apply_decision(decider, Decision::Approve, ""),
                KeyCode::Char('N') => app.apply_decision(decider, Decision::Deny, ""),
                KeyCode::Char('j') | KeyCode::Down => app.cycle_approval_next(),
                KeyCode::Char('k') | KeyCode::Up => app.cycle_approval_prev(),
                KeyCode::Esc | KeyCode::Char('q') => app.close_approvals_modal(),
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
        Stage::ApprovalsModal => {
            render_main(app, area, &mut buf);
            render_approvals_modal(area, &mut buf, app);
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
    use crate::data::AgentInfo;
    use crossterm::event::{KeyEvent, KeyEventState, KeyModifiers};
    use team_core::supervisor::AgentState;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    /// Noop decider for tests that don't exercise approve/deny.
    struct NoopDecider;
    impl crate::approvals::ApprovalDecider for NoopDecider {
        fn decide(
            &self,
            _root: &std::path::Path,
            _id: i64,
            _kind: crate::approvals::Decision,
            _note: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Boilerplate-free dispatcher for tests not exercising the
    /// decision path.
    fn dispatch(app: &mut App, ev: Event) {
        super::handle_event(app, ev, &NoopDecider);
    }

    fn agent(id: &str, state: AgentState) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            agent: id
                .split_once(':')
                .map(|(_, a)| a.to_string())
                .unwrap_or_default(),
            project: id
                .split_once(':')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default(),
            tmux_session: format!("t-{}", id.replace(':', "-")),
            state,
            unread_mail: 0,
            pending_approvals: 0,
            is_manager: false,
        }
    }

    pub fn fixture_team(agents: Vec<AgentInfo>) -> TeamSnapshot {
        TeamSnapshot {
            root: std::path::PathBuf::from("/fixture"),
            team_name: "fixture".into(),
            agents,
        }
    }

    #[test]
    fn splash_dismissed_by_any_key() {
        let mut app = App::new();
        assert_eq!(app.stage, Stage::Splash);
        dispatch(&mut app, key(KeyCode::Char(' ')));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn tab_cycles_focus_until_mailbox_then_cycles_mailbox_tabs() {
        // PR-UI-3: Tab still cycles panes Roster → Detail →
        // Mailbox, but once focused on Mailbox it cycles the
        // mailbox subtabs (Inbox → Channel → Wire) instead of
        // looping back to Roster. Shift+Tab pane reversal lands in
        // a later PR.
        let mut app = App::new();
        app.dismiss_splash();
        assert_eq!(app.focused_pane, Pane::Roster);
        dispatch(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Detail);
        dispatch(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Mailbox);
        assert_eq!(app.mailbox_tab, MailboxTab::Inbox);
        dispatch(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Mailbox, "still on mailbox");
        assert_eq!(app.mailbox_tab, MailboxTab::Channel);
        dispatch(&mut app, key(KeyCode::Tab));
        assert_eq!(app.mailbox_tab, MailboxTab::Wire);
        dispatch(&mut app, key(KeyCode::Tab));
        assert_eq!(app.mailbox_tab, MailboxTab::Inbox, "tabs wrap");
    }

    #[test]
    fn q_opens_confirm_then_n_cancels() {
        let mut app = App::new();
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.stage, Stage::QuitConfirm);
        dispatch(&mut app, key(KeyCode::Char('n')));
        assert_eq!(app.stage, Stage::Triptych);
        assert!(app.running, "n must not exit");
    }

    #[test]
    fn q_then_y_exits() {
        let mut app = App::new();
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('q')));
        dispatch(&mut app, key(KeyCode::Char('y')));
        assert!(!app.running);
    }

    #[test]
    fn esc_cancels_quit_confirm() {
        let mut app = App::new();
        app.dismiss_splash();
        app.enter_quit_confirm();
        dispatch(&mut app, key(KeyCode::Esc));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn render_does_not_panic_at_minimal_size() {
        let app = App::new();
        let _ = render_to_buffer(&app, 20, 8);
    }

    #[test]
    fn render_does_not_panic_at_huge_size() {
        let app = App::new();
        let _ = render_to_buffer(&app, 240, 80);
    }

    #[test]
    fn select_next_wraps_through_team() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
            agent("p:c", AgentState::Running),
        ]));
        assert_eq!(app.selected_agent, Some(0));
        app.select_next();
        assert_eq!(app.selected_agent, Some(1));
        app.select_next();
        assert_eq!(app.selected_agent, Some(2));
        app.select_next();
        assert_eq!(app.selected_agent, Some(0)); // wraps
    }

    #[test]
    fn select_prev_wraps_at_top() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
        ]));
        app.selected_agent = Some(0);
        app.select_prev();
        assert_eq!(app.selected_agent, Some(1));
    }

    #[test]
    fn select_no_op_on_empty_team() {
        let mut app = App::new();
        app.select_next();
        assert_eq!(app.selected_agent, None);
        app.select_prev();
        assert_eq!(app.selected_agent, None);
    }

    #[test]
    fn replace_team_preserves_selection_when_agent_still_present() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
        ]));
        app.selected_agent = Some(1);
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Stopped), // same id, new state
        ]));
        assert_eq!(app.selected_agent, Some(1), "selection follows the id");
    }

    #[test]
    fn replace_team_resets_selection_when_agent_disappears() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:gone", AgentState::Running),
        ]));
        app.selected_agent = Some(1);
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        assert_eq!(app.selected_agent, Some(0), "falls back to first agent");
    }

    #[test]
    fn switching_agent_resets_mailbox_buffers() {
        // The mailbox cursors are per-agent context; switching to a
        // new agent must clear them so we don't skip historical
        // rows that landed before the new agent's first refresh.
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
        ]));
        app.mailbox.extend(
            crate::mailbox::MailboxTab::Inbox,
            vec![crate::mailbox::MessageRow {
                id: 7,
                sender: "p:b".into(),
                recipient: "p:a".into(),
                text: "hi".into(),
                sent_at: 0.0,
            }],
        );
        assert_eq!(app.mailbox.inbox.len(), 1);
        assert_eq!(app.mailbox.inbox_after, 7);
        // Move selection to p:b — different agent id, mailbox resets.
        app.select_next();
        assert_eq!(app.selected_agent_id().as_deref(), Some("p:b"));
        assert!(app.mailbox.inbox.is_empty());
        assert_eq!(app.mailbox.inbox_after, 0);
    }

    /// Tiny single-call mailbox stub for the refresh-fanout test —
    /// keeps the assertion local without depending on
    /// `mailbox::tests::MockMailboxSource` (which lives behind a
    /// private `tests` module).
    struct TripleFilterMock {
        inbox: Vec<crate::mailbox::MessageRow>,
        channel: Vec<crate::mailbox::MessageRow>,
        wire: Vec<crate::mailbox::MessageRow>,
        calls: std::sync::Mutex<Vec<(&'static str, String, i64)>>,
    }
    impl crate::mailbox::MailboxSource for TripleFilterMock {
        fn inbox(&self, id: &str, after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            self.calls.lock().unwrap().push(("inbox", id.into(), after));
            Ok(self.inbox.clone())
        }
        fn channel_feed(
            &self,
            id: &str,
            after: i64,
        ) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            self.calls
                .lock()
                .unwrap()
                .push(("channel", id.into(), after));
            Ok(self.channel.clone())
        }
        fn wire(&self, id: &str, after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            self.calls.lock().unwrap().push(("wire", id.into(), after));
            Ok(self.wire.clone())
        }
    }

    #[test]
    fn refresh_mailbox_fans_out_to_three_filters() {
        use crate::mailbox::MessageRow;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        let mock = TripleFilterMock {
            inbox: vec![MessageRow {
                id: 1,
                sender: "p:b".into(),
                recipient: "p:a".into(),
                text: "dm".into(),
                sent_at: 0.0,
            }],
            channel: vec![MessageRow {
                id: 2,
                sender: "p:b".into(),
                recipient: "channel:p:editorial".into(),
                text: "ch".into(),
                sent_at: 0.0,
            }],
            wire: vec![MessageRow {
                id: 3,
                sender: "p:b".into(),
                recipient: "channel:p:all".into(),
                text: "wire".into(),
                sent_at: 0.0,
            }],
            calls: std::sync::Mutex::new(Vec::new()),
        };
        super::refresh_mailbox(&mut app, &mock);
        assert_eq!(app.mailbox.inbox.len(), 1);
        assert_eq!(app.mailbox.channel.len(), 1);
        assert_eq!(app.mailbox.wire.len(), 1);
        let calls = mock.calls.lock().unwrap();
        // The selected agent is p:a (auto-set by replace_team to
        // index 0); the wire filter takes the project id `p`.
        assert!(calls.contains(&("inbox", "p:a".into(), 0)));
        assert!(calls.contains(&("channel", "p:a".into(), 0)));
        assert!(calls.contains(&("wire", "p".into(), 0)));
    }

    fn ap(id: i64) -> crate::approvals::Approval {
        crate::approvals::Approval {
            id,
            project_id: "p".into(),
            agent_id: "p:m".into(),
            action: "publish".into(),
            summary: format!("approval #{id}"),
            payload_json: String::new(),
        }
    }

    #[test]
    fn has_pending_approvals_tracks_replace_calls() {
        let mut app = App::new();
        assert!(!app.has_pending_approvals());
        app.replace_approvals(vec![ap(1), ap(2)]);
        assert!(app.has_pending_approvals());
        app.replace_approvals(vec![]);
        assert!(!app.has_pending_approvals());
    }

    #[test]
    fn enter_approvals_modal_no_op_when_queue_empty() {
        let mut app = App::new();
        app.dismiss_splash();
        app.enter_approvals_modal();
        assert_eq!(app.stage, Stage::Triptych, "no pending → no modal");
    }

    #[test]
    fn a_chord_opens_modal_when_pending() {
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(1), ap(2)]);
        dispatch(&mut app, key(KeyCode::Char('a')));
        assert_eq!(app.stage, Stage::ApprovalsModal);
        assert_eq!(app.selected_approval, 0);
    }

    #[test]
    fn modal_cycle_jk_walks_approvals() {
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(1), ap(2), ap(3)]);
        app.enter_approvals_modal();
        dispatch(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.selected_approval, 1);
        dispatch(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.selected_approval, 2);
        dispatch(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.selected_approval, 0, "wraps");
        dispatch(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.selected_approval, 2, "k wraps too");
    }

    #[test]
    fn capital_y_routes_approve_through_decider() {
        use crate::approvals::test_support::MockApprovalDecider;
        let dec = MockApprovalDecider::default();
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(7), ap(8)]);
        app.enter_approvals_modal();
        super::handle_event(&mut app, key(KeyCode::Char('Y')), &dec);
        let calls = dec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 7);
        assert_eq!(calls[0].1, crate::approvals::Decision::Approve);
        // Optimistic local removal — approval id 7 dropped.
        assert_eq!(app.pending_approvals.len(), 1);
        assert_eq!(app.pending_approvals[0].id, 8);
    }

    #[test]
    fn capital_n_routes_deny_through_decider() {
        use crate::approvals::test_support::MockApprovalDecider;
        let dec = MockApprovalDecider::default();
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(7)]);
        app.enter_approvals_modal();
        super::handle_event(&mut app, key(KeyCode::Char('N')), &dec);
        let calls = dec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, crate::approvals::Decision::Deny);
        // Queue empty after the only approval resolves → modal closes.
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn esc_closes_approvals_modal() {
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(1)]);
        app.enter_approvals_modal();
        dispatch(&mut app, key(KeyCode::Esc));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn shift_tab_cycles_panes_backward() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.dismiss_splash();
        assert_eq!(app.focused_pane, Pane::Roster);
        // Shift+Tab from Roster → Mailbox (the "back out of mailbox"
        // direction's mirror).
        dispatch(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.focused_pane, Pane::Mailbox);
        // Some terminals send Tab + SHIFT instead of BackTab.
        dispatch(&mut app, key_with(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane, Pane::Detail);
    }

    #[test]
    fn replace_approvals_clamps_selection_in_range() {
        let mut app = App::new();
        app.replace_approvals(vec![ap(1), ap(2), ap(3)]);
        app.selected_approval = 2;
        // Approval id 3 resolved out-of-band; new snapshot has 2 rows.
        app.replace_approvals(vec![ap(1), ap(2)]);
        assert_eq!(app.selected_approval, 1, "clamps to last index");
    }

    #[test]
    fn arrow_keys_navigate_only_when_roster_focused() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
        ]));
        app.dismiss_splash();
        // Focused pane is Roster → arrow cycles selection.
        app.selected_agent = Some(0);
        dispatch(&mut app, key(KeyCode::Down));
        assert_eq!(app.selected_agent, Some(1));
        // Cycle to Detail → arrow no longer touches selection.
        app.cycle_focus();
        dispatch(&mut app, key(KeyCode::Down));
        assert_eq!(
            app.selected_agent,
            Some(1),
            "non-roster focus ignores arrows"
        );
    }
}
