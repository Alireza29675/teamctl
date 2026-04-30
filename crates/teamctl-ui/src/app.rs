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
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use crate::data::TeamSnapshot;
use crate::mailbox::{BrokerMailboxSource, MailboxBuffers, MailboxSource, MailboxTab};
use crate::pane::{PaneSource, TmuxPaneSource};
use crate::splash;
use crate::statusline;
use crate::theme::{detect_capabilities, Capabilities};
use crate::triptych::{self, Pane};
use crate::tutorial;

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
        }
    }

    pub fn cycle_mailbox_tab(&mut self) {
        self.mailbox_tab = self.mailbox_tab.next();
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
pub fn refresh<P: PaneSource, M: MailboxSource>(
    app: &mut App,
    pane_source: &P,
    mailbox_source: &M,
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
    app.last_refresh = Instant::now();
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
    // The broker DB path is `<root>/state/mailbox.db`; we don't
    // know the root until after the first snapshot load, so build
    // the source after refresh — but the source is cheap to
    // construct, so we just make a fresh one each tick keyed on
    // the current team root.
    refresh_with_default_sources(&mut app, &pane_source);
    while app.running {
        terminal.draw(|f| draw(f, &app))?;
        if event::poll(POLL_INTERVAL)? {
            handle_event(&mut app, event::read()?);
        }
        if matches!(app.stage, Stage::Splash) && app.splash_started.elapsed() >= SPLASH_AUTO_DISMISS
        {
            app.dismiss_splash();
        }
        if app.last_refresh.elapsed() >= REFRESH_INTERVAL {
            refresh_with_default_sources(&mut app, &pane_source);
        }
    }
    Ok(())
}

/// Build the production `BrokerMailboxSource` from the current
/// team root and run a refresh with both default sources. Lives
/// here (rather than inline in `run`) so the team-root → DB-path
/// derivation has one home.
fn refresh_with_default_sources<P: PaneSource>(app: &mut App, pane_source: &P) {
    if let Ok(Some(snapshot)) = TeamSnapshot::discover_and_load() {
        app.replace_team(snapshot);
    }
    let db_path = app.team.root.join("state/mailbox.db");
    let mailbox_source = BrokerMailboxSource::new(db_path);
    if let Some(session) = app.focused_session().map(|s| s.to_string()) {
        if let Ok(lines) = pane_source.capture(&session) {
            app.set_detail_buffer(lines);
        }
    } else {
        app.detail_buffer.clear();
    }
    refresh_mailbox(app, &mailbox_source);
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
        handle_event(&mut app, key(KeyCode::Char(' ')));
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
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Detail);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Mailbox);
        assert_eq!(app.mailbox_tab, MailboxTab::Inbox);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_pane, Pane::Mailbox, "still on mailbox");
        assert_eq!(app.mailbox_tab, MailboxTab::Channel);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.mailbox_tab, MailboxTab::Wire);
        handle_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.mailbox_tab, MailboxTab::Inbox, "tabs wrap");
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
        handle_event(&mut app, key(KeyCode::Down));
        assert_eq!(app.selected_agent, Some(1));
        // Cycle to Detail → arrow no longer touches selection.
        app.cycle_focus();
        handle_event(&mut app, key(KeyCode::Down));
        assert_eq!(
            app.selected_agent,
            Some(1),
            "non-roster focus ignores arrows"
        );
    }
}
