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
use crate::compose::{CliMessageSender, ComposeTarget, Editor, EditorAction, MessageSender};
use crate::data::TeamSnapshot;
use crate::layouts;
use crate::mailbox::{BrokerMailboxSource, MailboxBuffers, MailboxSource, MailboxTab};
use crate::pane::{PaneSource, TmuxPaneSource};
use crate::splash;
use crate::statusline;
use crate::theme::{detect_capabilities, Capabilities};
use crate::triptych::{self, MainLayout, Pane};
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
    /// Compose modal — opens on `@` (DM-to-focused-agent) or `!`
    /// (broadcast-to-current-channel). Routes through `teamctl
    /// send|broadcast` so the channel-ACL + ratelimit + delivery
    /// hooks the CLI already runs through ride for free.
    ComposeModal,
    /// `?` help overlay — modal listing every chord registered in
    /// `help::ALL_GROUPS`. Read-only; closes on Esc / `?`.
    HelpOverlay,
    /// Onboarding tutorial walkthrough. Auto-opens on first
    /// launch (per-team sentinel at
    /// `.team/state/ui-tutorial-completed`); reopenable via `t`
    /// from any non-modal state.
    Tutorial,
}

/// Splitscreen orientation per detail-pane split (PR-UI-7 lift
/// of PR-UI-6's deferred Q1). `Vertical` subdivides side-by-side
/// (Ctrl+|); `Horizontal` stacks top-to-bottom (Ctrl+-).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
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
    /// Open compose target — `Some` while `Stage::ComposeModal`
    /// is the active stage, `None` otherwise. Stored on App so
    /// the editor's contents survive rerenders.
    pub compose_target: Option<ComposeTarget>,
    /// Editor backing the compose modal. Reset to `default()` each
    /// time the modal opens so an old draft from a prior
    /// invocation can't leak into a new send.
    pub compose_editor: Editor,
    /// Last error from a CLI-routed send call — surfaced inline
    /// in the modal so the operator sees rate-limit / ACL-block
    /// errors without leaving the UI.
    pub compose_error: Option<String>,
    /// Active main-view layout (PR-UI-6). Triptych is the default;
    /// `Ctrl+W` toggles Wall, `Ctrl+M` toggles MailboxFirst.
    pub layout: MainLayout,
    /// Top-of-window agent index for the Wall view's vertical
    /// scroll. SPEC §3 caps visible tiles at 4; this offsets which
    /// 4-agent window is shown when the team has more.
    pub wall_scroll: usize,
    /// Selected channel index (into `team.channels`) for the
    /// MailboxFirst layout's channel list and for the broadcast
    /// picker. `None` until the operator picks one.
    pub selected_channel: Option<usize>,
    /// Splits within Triptych's detail pane (PR-UI-6). When
    /// non-empty, the detail pane subdivides; each entry pairs an
    /// agent id with the per-split orientation (PR-UI-7 lift of
    /// the Q1 deferral). `selected_split` is the vim-window-motion
    /// focus.
    pub detail_splits: Vec<(String, SplitOrientation)>,
    pub selected_split: usize,
    /// Chord-prefix machine for `Ctrl+W` follow-ups (PR-UI-7 lift
    /// of PR-UI-6's `Ctrl+Q` alias). When `Some(KeyCode::Char('w'))`,
    /// the next key is interpreted as a `Ctrl+W` follow: `q` =
    /// close split, `o` = close others. Cleared on any unrelated
    /// keypress so a typo doesn't leave the editor stuck.
    pub pending_chord: Option<KeyCode>,
    /// `true` when the operator's first launch on this team has
    /// not yet completed the tutorial — drives the auto-open after
    /// splash. Reset to `false` on tutorial completion.
    pub tutorial_pending_for_team: bool,
    /// Brand-spinner frame counter (PR-UI-7). Bumped each refresh
    /// tick so the statusline indicator shows the app is alive.
    pub spinner_frame: usize,
    /// Tutorial step cursor (PR-UI-7). Index into
    /// `onboarding::STEPS`; reset to 0 when the tutorial reopens.
    pub tutorial_step: usize,
    /// Modal substage for the broadcast channel picker (PR-UI-6).
    /// When `true` the compose modal renders a picker over the
    /// editor; selecting a channel populates `compose_target` and
    /// drops back to the editor.
    pub compose_picker_open: bool,
    /// Picker selection cursor — index into `team.channels`.
    pub compose_picker_index: usize,
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
            compose_target: None,
            compose_editor: Editor::default(),
            compose_error: None,
            layout: MainLayout::Triptych,
            wall_scroll: 0,
            selected_channel: None,
            detail_splits: Vec::new(),
            selected_split: 0,
            compose_picker_open: false,
            compose_picker_index: 0,
            pending_chord: None,
            tutorial_pending_for_team: false,
            spinner_frame: 0,
            tutorial_step: 0,
        }
    }

    /// Per-tutorial-step cursor (used by Stage::Tutorial). Wraps
    /// at the end so `t`-then-keys walks the full tour.
    pub fn enter_help_overlay(&mut self) {
        self.previous_stage = self.stage;
        self.stage = Stage::HelpOverlay;
    }
    pub fn close_help_overlay(&mut self) {
        self.stage = self.previous_stage;
    }
    pub fn enter_tutorial(&mut self) {
        self.previous_stage = self.stage;
        self.stage = Stage::Tutorial;
        self.tutorial_step = 0;
    }
    pub fn close_tutorial(&mut self) {
        self.stage = self.previous_stage;
        self.tutorial_pending_for_team = false;
        if !self.team.root.as_os_str().is_empty() {
            let _ = crate::onboarding::mark_completed(&self.team.root);
        }
    }
    pub fn tutorial_advance(&mut self) {
        let len = crate::onboarding::STEPS.len();
        if len == 0 {
            self.close_tutorial();
            return;
        }
        if self.tutorial_step + 1 >= len {
            self.close_tutorial();
        } else {
            self.tutorial_step += 1;
        }
    }
    pub fn tutorial_back(&mut self) {
        self.tutorial_step = self.tutorial_step.saturating_sub(1);
    }

    pub fn toggle_wall_layout(&mut self) {
        self.layout = self.layout.toggle_wall();
    }
    pub fn toggle_mailbox_first_layout(&mut self) {
        self.layout = self.layout.toggle_mailbox_first();
        // First entry into MailboxFirst seeds the channel cursor
        // so the feed pane has something to render.
        if matches!(self.layout, MainLayout::MailboxFirst) && self.selected_channel.is_none() {
            self.selected_channel = if self.team.channels.is_empty() {
                None
            } else {
                Some(0)
            };
        }
    }
    pub fn wall_scroll_up(&mut self) {
        self.wall_scroll = self
            .wall_scroll
            .saturating_sub(crate::layouts::WALL_TILE_CAP);
    }
    pub fn wall_scroll_down(&mut self) {
        let next = self.wall_scroll + crate::layouts::WALL_TILE_CAP;
        if next < self.team.agents.len() {
            self.wall_scroll = next;
        }
    }
    pub fn select_next_channel(&mut self) {
        if self.team.channels.is_empty() {
            return;
        }
        self.selected_channel = Some(match self.selected_channel {
            None => 0,
            Some(i) => (i + 1) % self.team.channels.len(),
        });
    }
    pub fn select_prev_channel(&mut self) {
        if self.team.channels.is_empty() {
            return;
        }
        self.selected_channel = Some(match self.selected_channel {
            None | Some(0) => self.team.channels.len() - 1,
            Some(i) => i - 1,
        });
    }

    /// Add a split for the focused agent (or current selection)
    /// to the detail pane. Cap at 4 splits per the SPEC §3 cap.
    /// Add a vertical split (PR-UI-7). `Ctrl+|` calls this.
    pub fn add_detail_split_vertical(&mut self) {
        self.add_detail_split_with_orientation(SplitOrientation::Vertical);
    }
    /// Add a horizontal split (PR-UI-7). `Ctrl+-` calls this.
    pub fn add_detail_split_horizontal(&mut self) {
        self.add_detail_split_with_orientation(SplitOrientation::Horizontal);
    }
    fn add_detail_split_with_orientation(&mut self, orientation: SplitOrientation) {
        let Some(id) = self.selected_agent_id() else {
            return;
        };
        if self.detail_splits.len() >= 4 {
            return;
        }
        self.detail_splits.push((id, orientation));
        self.selected_split = self.detail_splits.len() - 1;
    }
    /// Back-compat shim — earlier PRs called the unsuffixed name.
    /// Defaults to vertical (matching the most-common chord
    /// `Ctrl+|`). Kept so the test surface PR-UI-6 pinned doesn't
    /// drift.
    pub fn add_detail_split(&mut self) {
        self.add_detail_split_vertical();
    }
    pub fn close_focused_split(&mut self) {
        if self.detail_splits.is_empty() {
            return;
        }
        let i = self.selected_split.min(self.detail_splits.len() - 1);
        self.detail_splits.remove(i);
        self.selected_split = i.saturating_sub(1);
    }
    pub fn cycle_split_next(&mut self) {
        if self.detail_splits.is_empty() {
            return;
        }
        self.selected_split = (self.selected_split + 1) % self.detail_splits.len();
    }
    pub fn cycle_split_prev(&mut self) {
        if self.detail_splits.is_empty() {
            return;
        }
        self.selected_split = if self.selected_split == 0 {
            self.detail_splits.len() - 1
        } else {
            self.selected_split - 1
        };
    }

    /// Open the broadcast compose flow — picker first when at
    /// least one channel is declared, else fall back to the
    /// project's `all` channel (PR-UI-5 behaviour) on the
    /// assumption that `all` always exists in production composes.
    pub fn enter_compose_broadcast_with_picker(&mut self) {
        if self.team.channels.is_empty() {
            // Fall back to the PR-UI-5 default if no channels
            // are declared yet — should only happen with a
            // half-loaded snapshot.
            self.enter_compose_broadcast();
            return;
        }
        let project_id = self
            .team
            .channels
            .first()
            .map(|c| c.project_id.clone())
            .unwrap_or_default();
        self.previous_stage = self.stage;
        self.stage = Stage::ComposeModal;
        self.compose_target = Some(ComposeTarget::Broadcast {
            channel_id: format!("{project_id}:all"),
            project_id,
        });
        self.compose_editor = Editor::default();
        self.compose_error = None;
        self.compose_picker_open = true;
        self.compose_picker_index = 0;
    }
    pub fn picker_next(&mut self) {
        if self.team.channels.is_empty() {
            return;
        }
        self.compose_picker_index = (self.compose_picker_index + 1) % self.team.channels.len();
    }
    pub fn picker_prev(&mut self) {
        if self.team.channels.is_empty() {
            return;
        }
        self.compose_picker_index = if self.compose_picker_index == 0 {
            self.team.channels.len() - 1
        } else {
            self.compose_picker_index - 1
        };
    }
    pub fn picker_confirm(&mut self) {
        if let Some(ch) = self.team.channels.get(self.compose_picker_index) {
            self.compose_target = Some(ComposeTarget::Broadcast {
                channel_id: ch.id.clone(),
                project_id: ch.project_id.clone(),
            });
        }
        self.compose_picker_open = false;
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

    /// Open the compose modal for the focused agent (if any). The
    /// `@` chord. No-op when no agent is focused.
    pub fn enter_compose_dm_for_focused(&mut self) {
        let Some(info) = self
            .selected_agent
            .and_then(|i| self.team.agents.get(i))
            .cloned()
        else {
            return;
        };
        self.previous_stage = self.stage;
        self.stage = Stage::ComposeModal;
        self.compose_target = Some(ComposeTarget::Dm {
            agent_id: info.id.clone(),
            project_id: info.project.clone(),
        });
        self.compose_editor = Editor::default();
        self.compose_error = None;
    }

    /// Open the compose modal targeting the project's `all`
    /// channel — the broadcast wire. The `!` chord. PR-UI-5 ships
    /// with channel scoping limited to `all` (the Wire tab is the
    /// only channel context the mailbox pane currently surfaces);
    /// PR-UI-6's mailbox UI work will widen the scope to per-channel
    /// targets when individual channels become first-class in the
    /// pane.
    pub fn enter_compose_broadcast(&mut self) {
        let project_id = self
            .selected_agent
            .and_then(|i| self.team.agents.get(i))
            .map(|a| a.project.clone())
            .or_else(|| self.team.agents.first().map(|a| a.project.clone()));
        let Some(project_id) = project_id else {
            return;
        };
        let channel_id = format!("{project_id}:all");
        self.previous_stage = self.stage;
        self.stage = Stage::ComposeModal;
        self.compose_target = Some(ComposeTarget::Broadcast {
            channel_id,
            project_id,
        });
        self.compose_editor = Editor::default();
        self.compose_error = None;
    }

    pub fn close_compose_modal(&mut self) {
        self.stage = self.previous_stage;
        self.compose_target = None;
        self.compose_editor = Editor::default();
        self.compose_error = None;
    }

    /// Send the current compose body via the injected message
    /// sender. Routes through `teamctl send|broadcast` in
    /// production; tests inject a recorder. Closes the modal +
    /// triggers a mailbox refresh on success; surfaces error
    /// inline on failure.
    pub fn apply_send<S: MessageSender, M: MailboxSource>(
        &mut self,
        sender: &S,
        mailbox_source: &M,
    ) {
        let Some(target) = self.compose_target.clone() else {
            return;
        };
        let body = self.compose_editor.body();
        if body.is_empty() {
            self.compose_error = Some("body is empty".into());
            return;
        }
        let result = match &target {
            ComposeTarget::Dm { agent_id, .. } => sender.send_dm(&self.team.root, agent_id, &body),
            ComposeTarget::Broadcast { channel_id, .. } => {
                sender.broadcast(&self.team.root, channel_id, &body)
            }
        };
        match result {
            Ok(()) => {
                self.close_compose_modal();
                // Refresh the mailbox so the just-sent row appears
                // in the relevant tab on the next paint.
                refresh_mailbox(self, mailbox_source);
            }
            Err(err) => {
                self.compose_error = Some(err.to_string());
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
    let sender = CliMessageSender;
    // First refresh resolves the team root; only then can we
    // bring up the file-watcher, which keys on `<root>/state/`.
    refresh_with_default_sources(&mut app, &pane_source);
    let mut watch = Watch::try_new(&app.team.root.join("state"));
    while app.running {
        terminal.draw(|f| draw(f, &app))?;
        if event::poll(POLL_INTERVAL)? {
            // The mailbox source for handle_event mirrors the
            // refresh path; the same db_path key avoids divergence
            // between read + write fanout.
            let db_path = app.team.root.join("state/mailbox.db");
            let mailbox_source = BrokerMailboxSource::new(db_path);
            handle_event(&mut app, event::read()?, &decider, &sender, &mailbox_source);
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
        Stage::ComposeModal => {
            draw_main(f, area, app);
            draw_compose_modal(f, area, app);
        }
        Stage::HelpOverlay => {
            draw_main(f, area, app);
            let buf = f.buffer_mut();
            render_help_overlay(area, buf, app);
        }
        Stage::Tutorial => {
            draw_main(f, area, app);
            let buf = f.buffer_mut();
            render_tutorial(area, buf, app);
        }
    }
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

fn draw_main(f: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let buf = f.buffer_mut();
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
}

fn draw_approvals_modal(f: &mut Frame<'_>, area: Rect, app: &App) {
    let buf = f.buffer_mut();
    render_approvals_modal(area, buf, app);
}

fn draw_compose_modal(f: &mut Frame<'_>, area: Rect, app: &App) {
    let buf = f.buffer_mut();
    render_compose_modal(area, buf, app);
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
    Paragraph::new(lines).render(chunks[0], buf);
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
        .map(|t| t.title())
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
                let col = app.compose_editor.cursor_col.min(line.len());
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
    Paragraph::new(body_lines).render(chunks[0], buf);

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

    Paragraph::new("Ctrl+Enter send · Esc Esc cancel · Tab attach (TODO #32)")
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
        "[y] approve  ·  [Shift-N] deny  ·  [j/k] cycle  ·  [Esc] close",
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

fn handle_event<D: ApprovalDecider, S: MessageSender, M: MailboxSource>(
    app: &mut App,
    ev: Event,
    decider: &D,
    sender: &S,
    mailbox_source: &M,
) {
    use crossterm::event::KeyModifiers;
    match ev {
        Event::Key(k) if k.kind == KeyEventKind::Press => match app.stage {
            Stage::Splash => app.dismiss_splash(),
            Stage::Triptych => match k.code {
                // PR-UI-7 chord-prefix follow-ups MUST be tested
                // before unguarded `Char('q')` / `Char('o')` arms,
                // otherwise the no-modifier `q` quit would shadow
                // the `Ctrl+W q` close-split.
                KeyCode::Char('q') if app.pending_chord == Some(KeyCode::Char('w')) => {
                    app.pending_chord = None;
                    app.close_focused_split();
                }
                KeyCode::Char('o') if app.pending_chord == Some(KeyCode::Char('w')) => {
                    app.pending_chord = None;
                    if !app.detail_splits.is_empty() {
                        let keep = app.selected_split.min(app.detail_splits.len() - 1);
                        let kept = app.detail_splits.remove(keep);
                        app.detail_splits.clear();
                        app.detail_splits.push(kept);
                        app.selected_split = 0;
                    }
                }
                KeyCode::Char('q') => app.enter_quit_confirm(),
                // PR-UI-4: `a` opens the approvals modal when there's
                // at least one pending row. No-op otherwise so the
                // chord doesn't surprise anyone hammering keys.
                KeyCode::Char('a') => app.enter_approvals_modal(),
                // PR-UI-5: `@` opens DM compose to focused agent.
                // PR-UI-6: `!` now opens the broadcast picker so
                // operators choose which channel to broadcast to,
                // not just the project's `all` wire.
                KeyCode::Char('@') => app.enter_compose_dm_for_focused(),
                KeyCode::Char('!') => app.enter_compose_broadcast_with_picker(),
                // PR-UI-7 chord-prefix: when there's at least one
                // detail split, `Ctrl+W` arms the chord-prefix
                // (the next key dispatches `q` close-split, `o`
                // close-others). Tested BEFORE the wall-layout
                // toggle below so the chord-prefix wins when
                // relevant.
                KeyCode::Char('w')
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.detail_splits.is_empty() =>
                {
                    app.pending_chord = Some(KeyCode::Char('w'))
                }
                // PR-UI-6: layout toggles. `Ctrl+W` for Wall when
                // there are no splits to chord on; `Ctrl+M` for
                // MailboxFirst (always).
                KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.toggle_wall_layout()
                }
                KeyCode::Char('m') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.toggle_mailbox_first_layout()
                }
                // PR-UI-7 splitscreen lift: `Ctrl+|` subdivides
                // vertically, `Ctrl+-` horizontally — vim/tmux
                // operators' muscle memory matches the visual.
                KeyCode::Char('|') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.add_detail_split_vertical()
                }
                KeyCode::Char('-') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.add_detail_split_horizontal()
                }
                // Vim window-motion `Ctrl+H/J/K/L` cycles between
                // splits when there's more than one.
                KeyCode::Char('h') | KeyCode::Char('k')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.cycle_split_prev()
                }
                KeyCode::Char('l') | KeyCode::Char('j')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.cycle_split_next()
                }
                // PR-UI-6 alias preserved for back-compat: `Ctrl+Q`
                // closes the focused split. PR-UI-7 also wires the
                // proper `Ctrl+W q` chord; both work.
                KeyCode::Char('Q') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.close_focused_split()
                }
                // (chord-prefix follow-ups handled at top of arm
                // before unguarded letter-arms — see top of
                // `Stage::Triptych` match.)
                // PR-UI-7 help + tutorial chords. `?` opens help
                // overlay; `t` reopens tutorial. Both no-op if a
                // modifier is in flight (so `Shift+?` and `Ctrl+T`
                // don't double-bind).
                KeyCode::Char('?')
                    if k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT =>
                {
                    app.enter_help_overlay()
                }
                KeyCode::Char('t') if k.modifiers.is_empty() => app.enter_tutorial(),
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
                // PR-UI-6: in Wall layout, `j`/`k` (and arrows)
                // scroll the tile grid — same vim shape, different
                // surface. In Triptych roster focus they still
                // navigate the roster.
                KeyCode::Up | KeyCode::Char('k') if matches!(app.layout, MainLayout::Wall) => {
                    app.wall_scroll_up()
                }
                KeyCode::Down | KeyCode::Char('j') if matches!(app.layout, MainLayout::Wall) => {
                    app.wall_scroll_down()
                }
                // PR-UI-6: in MailboxFirst, `j`/`k` walk the
                // channel list.
                KeyCode::Up | KeyCode::Char('k')
                    if matches!(app.layout, MainLayout::MailboxFirst) =>
                {
                    app.select_prev_channel()
                }
                KeyCode::Down | KeyCode::Char('j')
                    if matches!(app.layout, MainLayout::MailboxFirst) =>
                {
                    app.select_next_channel()
                }
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
                // Asymmetric chord shape (T-074 bug 4 fix): approve is
                // the common path so it accepts both `y` and `Y` —
                // matches QuitConfirm's loose convention and the
                // muscle-memory most TUI prompts build. Deny is the
                // destructive side, so it requires deliberate Shift
                // (`N` only); a stray lowercase `n` does nothing.
                // Trades cosmetic chord-symmetry for discoverability
                // on the load-bearing approve flow.
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.apply_decision(decider, Decision::Approve, "")
                }
                KeyCode::Char('N') => app.apply_decision(decider, Decision::Deny, ""),
                KeyCode::Char('j') | KeyCode::Down => app.cycle_approval_next(),
                KeyCode::Char('k') | KeyCode::Up => app.cycle_approval_prev(),
                KeyCode::Esc | KeyCode::Char('q') => app.close_approvals_modal(),
                _ => {}
            },
            Stage::ComposeModal => {
                // PR-UI-6: when the broadcast picker is open the
                // editor doesn't see keys yet — operator first
                // chooses a channel.
                if app.compose_picker_open {
                    match k.code {
                        KeyCode::Down | KeyCode::Char('j') => app.picker_next(),
                        KeyCode::Up | KeyCode::Char('k') => app.picker_prev(),
                        KeyCode::Enter => app.picker_confirm(),
                        // PR-UI-6 fixup (Q6, dev2 review): Esc
                        // dismisses the picker overlay only and
                        // returns to the editor with whatever the
                        // operator already typed; the editor's own
                        // Esc-Esc cancel-the-modal flow handles
                        // bailing out of the whole compose. Mirrors
                        // the overlay-vs-modal symmetry vim users
                        // expect.
                        KeyCode::Esc => {
                            app.compose_picker_open = false;
                            app.compose_picker_index = 0;
                        }
                        _ => {}
                    }
                } else {
                    // Route every keypress through the editor; the
                    // editor returns Send / Cancel / Continue.
                    match app.compose_editor.apply_key(k) {
                        EditorAction::Continue => {}
                        EditorAction::Send => app.apply_send(sender, mailbox_source),
                        EditorAction::Cancel => app.close_compose_modal(),
                    }
                }
            }
            Stage::HelpOverlay => match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => app.close_help_overlay(),
                _ => {}
            },
            Stage::Tutorial => match k.code {
                KeyCode::Esc => app.close_tutorial(),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('p') => app.tutorial_back(),
                _ => app.tutorial_advance(),
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
        Stage::ComposeModal => {
            render_main(app, area, &mut buf);
            render_compose_modal(area, &mut buf, app);
        }
        Stage::HelpOverlay => {
            render_main(app, area, &mut buf);
            render_help_overlay(area, &mut buf, app);
        }
        Stage::Tutorial => {
            render_main(app, area, &mut buf);
            render_tutorial(area, &mut buf, app);
        }
    }
    buf
}

fn render_main(app: &App, area: Rect, buf: &mut Buffer) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
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

    /// Noop sender for tests that don't exercise compose-send.
    struct NoopSender;
    impl crate::compose::MessageSender for NoopSender {
        fn send_dm(
            &self,
            _root: &std::path::Path,
            _agent: &str,
            _body: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn broadcast(
            &self,
            _root: &std::path::Path,
            _channel: &str,
            _body: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Mailbox source that returns nothing — refresh_mailbox after
    /// a successful send becomes a no-op.
    struct EmptyMailbox;
    impl crate::mailbox::MailboxSource for EmptyMailbox {
        fn inbox(&self, _id: &str, _after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            Ok(Vec::new())
        }
        fn channel_feed(
            &self,
            _id: &str,
            _after: i64,
        ) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            Ok(Vec::new())
        }
        fn wire(&self, _id: &str, _after: i64) -> anyhow::Result<Vec<crate::mailbox::MessageRow>> {
            Ok(Vec::new())
        }
    }

    /// Boilerplate-free dispatcher for tests not exercising the
    /// decision / send paths.
    fn dispatch(app: &mut App, ev: Event) {
        super::handle_event(app, ev, &NoopDecider, &NoopSender, &EmptyMailbox);
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
            channels: Vec::new(),
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
        super::handle_event(
            &mut app,
            key(KeyCode::Char('Y')),
            &dec,
            &NoopSender,
            &EmptyMailbox,
        );
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
        super::handle_event(
            &mut app,
            key(KeyCode::Char('N')),
            &dec,
            &NoopSender,
            &EmptyMailbox,
        );
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
    fn lowercase_y_routes_approve_through_decider() {
        // T-074 bug 4: discoverable approve. Most operators try
        // lowercase first; the modal must accept it on the
        // approve (low-risk) side. Deny stays Shift-gated.
        use crate::approvals::test_support::MockApprovalDecider;
        let dec = MockApprovalDecider::default();
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(7)]);
        app.enter_approvals_modal();
        super::handle_event(
            &mut app,
            key(KeyCode::Char('y')),
            &dec,
            &NoopSender,
            &EmptyMailbox,
        );
        let calls = dec.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, crate::approvals::Decision::Approve);
    }

    #[test]
    fn lowercase_n_does_not_deny() {
        // Asymmetry guard: deny is destructive — `n` lowercase must
        // NOT fire the decider. A future "symmetric loose" refactor
        // would silently regress the destructive-deny Shift-gate;
        // this test pins it.
        use crate::approvals::test_support::MockApprovalDecider;
        let dec = MockApprovalDecider::default();
        let mut app = App::new();
        app.dismiss_splash();
        app.replace_approvals(vec![ap(7)]);
        app.enter_approvals_modal();
        super::handle_event(
            &mut app,
            key(KeyCode::Char('n')),
            &dec,
            &NoopSender,
            &EmptyMailbox,
        );
        assert!(
            dec.calls.lock().unwrap().is_empty(),
            "lowercase n must not route through the decider"
        );
        assert_eq!(
            app.stage,
            Stage::ApprovalsModal,
            "stale lowercase n leaves the modal open"
        );
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
    fn at_chord_opens_compose_dm_to_focused_agent() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("writing:manager", AgentState::Running),
            agent("writing:dev1", AgentState::Running),
        ]));
        app.dismiss_splash();
        app.select_next();
        dispatch(&mut app, key(KeyCode::Char('@')));
        assert_eq!(app.stage, Stage::ComposeModal);
        match app.compose_target.as_ref() {
            Some(crate::compose::ComposeTarget::Dm { agent_id, .. }) => {
                assert_eq!(agent_id, "writing:dev1");
            }
            other => panic!("expected DM target, got {other:?}"),
        }
    }

    #[test]
    fn bang_chord_opens_compose_broadcast_to_all_channel() {
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent(
            "writing:manager",
            AgentState::Running,
        )]));
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('!')));
        assert_eq!(app.stage, Stage::ComposeModal);
        match app.compose_target.as_ref() {
            Some(crate::compose::ComposeTarget::Broadcast { channel_id, .. }) => {
                assert_eq!(channel_id, "writing:all");
            }
            other => panic!("expected Broadcast target, got {other:?}"),
        }
    }

    #[test]
    fn send_routes_dm_through_mock_sender() {
        use crate::compose::test_support::MockMessageSender;
        let sender = MockMessageSender::default();
        let mailbox = EmptyMailbox;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent(
            "writing:dev1",
            AgentState::Running,
        )]));
        app.dismiss_splash();
        app.enter_compose_dm_for_focused();
        for c in "ship it".chars() {
            super::handle_event(
                &mut app,
                key(KeyCode::Char(c)),
                &NoopDecider,
                &sender,
                &mailbox,
            );
        }
        super::handle_event(
            &mut app,
            key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        let calls = sender.dm_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "writing:dev1");
        assert_eq!(calls[0].1, "ship it");
        assert_eq!(app.stage, Stage::Triptych, "modal closes on send");
    }

    #[test]
    fn esc_esc_cancels_compose_without_send() {
        use crate::compose::test_support::MockMessageSender;
        let sender = MockMessageSender::default();
        let mailbox = EmptyMailbox;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent(
            "writing:dev1",
            AgentState::Running,
        )]));
        app.dismiss_splash();
        app.enter_compose_dm_for_focused();
        for c in "draft".chars() {
            super::handle_event(
                &mut app,
                key(KeyCode::Char(c)),
                &NoopDecider,
                &sender,
                &mailbox,
            );
        }
        super::handle_event(&mut app, key(KeyCode::Esc), &NoopDecider, &sender, &mailbox);
        super::handle_event(&mut app, key(KeyCode::Esc), &NoopDecider, &sender, &mailbox);
        assert_eq!(app.stage, Stage::Triptych);
        assert!(sender.dm_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn send_failure_surfaces_error_inline_keeps_modal_open() {
        use crate::compose::test_support::MockMessageSender;
        let sender = MockMessageSender::default();
        *sender.fail_next.lock().unwrap() = Some("rate limit".into());
        let mailbox = EmptyMailbox;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent(
            "writing:dev1",
            AgentState::Running,
        )]));
        app.dismiss_splash();
        app.enter_compose_dm_for_focused();
        super::handle_event(
            &mut app,
            key(KeyCode::Char('x')),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        super::handle_event(
            &mut app,
            key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        assert_eq!(app.stage, Stage::ComposeModal, "modal stays open on err");
        assert!(app
            .compose_error
            .as_deref()
            .unwrap_or_default()
            .contains("rate limit"));
    }

    fn channel(id: &str, project: &str) -> crate::data::ChannelInfo {
        crate::data::ChannelInfo {
            id: id.into(),
            name: id
                .rsplit_once(':')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default(),
            project_id: project.into(),
        }
    }

    fn fixture_team_with_channels(
        agents: Vec<AgentInfo>,
        channels: Vec<crate::data::ChannelInfo>,
    ) -> TeamSnapshot {
        TeamSnapshot {
            root: std::path::PathBuf::from("/fixture"),
            team_name: "fixture".into(),
            agents,
            channels,
        }
    }

    #[test]
    fn ctrl_w_toggles_wall_layout() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.dismiss_splash();
        assert_eq!(app.layout, MainLayout::Triptych);
        dispatch(
            &mut app,
            key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.layout, MainLayout::Wall);
        dispatch(
            &mut app,
            key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.layout, MainLayout::Triptych);
    }

    #[test]
    fn ctrl_m_toggles_mailbox_first_layout() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.dismiss_splash();
        dispatch(
            &mut app,
            key_with(KeyCode::Char('m'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.layout, MainLayout::MailboxFirst);
        dispatch(
            &mut app,
            key_with(KeyCode::Char('m'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.layout, MainLayout::Triptych);
    }

    #[test]
    fn wall_scroll_pages_through_overflow_agents() {
        let mut app = App::new();
        let mut agents: Vec<_> = (1..=10)
            .map(|i| agent(&format!("p:agent-{i:02}"), AgentState::Running))
            .collect();
        // managers-first sort would otherwise reorder; mark all as workers.
        for a in agents.iter_mut() {
            a.is_manager = false;
        }
        app.replace_team(fixture_team(agents));
        app.dismiss_splash();
        app.toggle_wall_layout();
        assert_eq!(app.wall_scroll, 0);
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 4);
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 8);
        // Past 10-1 = 9; cap blocks 12.
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 8, "scroll capped at last full window");
        app.wall_scroll_up();
        assert_eq!(app.wall_scroll, 4);
    }

    #[test]
    fn ctrl_pipe_adds_detail_split_capped_at_four() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![
            agent("p:a", AgentState::Running),
            agent("p:b", AgentState::Running),
        ]));
        app.dismiss_splash();
        for _ in 0..6 {
            dispatch(
                &mut app,
                key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
            );
        }
        assert_eq!(app.detail_splits.len(), 4, "split count capped at 4");
    }

    #[test]
    fn ctrl_q_closes_focused_split() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        app.dismiss_splash();
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.detail_splits.len(), 2);
        dispatch(
            &mut app,
            key_with(KeyCode::Char('Q'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.detail_splits.len(), 1);
    }

    #[test]
    fn ctrl_hjkl_cycles_splits() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        app.dismiss_splash();
        for _ in 0..3 {
            dispatch(
                &mut app,
                key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
            );
        }
        assert_eq!(app.selected_split, 2);
        dispatch(
            &mut app,
            key_with(KeyCode::Char('l'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.selected_split, 0, "wraps");
        dispatch(
            &mut app,
            key_with(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.selected_split, 2);
    }

    #[test]
    fn wall_scroll_at_exactly_cap_agents_does_not_scroll() {
        // PR-UI-6 fixup (qa Gap 1a): with exactly WALL_TILE_CAP=4
        // agents the entire team fits in one window — scrolling
        // is a no-op in both directions. Pinning this catches a
        // future `<` → `<=` slip in `wall_scroll_down`.
        let mut app = App::new();
        let agents: Vec<_> = (1..=4)
            .map(|i| agent(&format!("p:agent-{i}"), AgentState::Running))
            .collect();
        app.replace_team(fixture_team(agents));
        app.dismiss_splash();
        app.toggle_wall_layout();
        assert_eq!(app.wall_scroll, 0);
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 0, "exactly-cap should not advance");
        app.wall_scroll_up();
        assert_eq!(app.wall_scroll, 0);
    }

    #[test]
    fn wall_scroll_at_cap_plus_one_advances_then_stops() {
        // PR-UI-6 fixup (qa Gap 1b): exactly 5 agents → 4 fit in
        // window-0, the 5th lives at window-4. One scroll
        // advances; the next caps. Pins the off-by-one between 4
        // and 5 agents.
        let mut app = App::new();
        let agents: Vec<_> = (1..=5)
            .map(|i| agent(&format!("p:agent-{i}"), AgentState::Running))
            .collect();
        app.replace_team(fixture_team(agents));
        app.dismiss_splash();
        app.toggle_wall_layout();
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 4, "first scroll exposes agent 5");
        app.wall_scroll_down();
        assert_eq!(app.wall_scroll, 4, "second scroll caps; nothing past");
    }

    #[test]
    fn esc_in_picker_dismisses_overlay_only_keeps_modal_open() {
        // PR-UI-6 fixup (Q6 dev2 review + qa Gap 3): Esc inside
        // the broadcast picker should close the picker overlay
        // and return to the editor in its current state — NOT
        // close the whole compose modal. Editor's Esc-Esc
        // already handles cancel-the-modal.
        let mut app = App::new();
        app.replace_team(fixture_team_with_channels(
            vec![agent("writing:manager", AgentState::Running)],
            vec![
                channel("writing:all", "writing"),
                channel("writing:editorial", "writing"),
            ],
        ));
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('!')));
        assert!(app.compose_picker_open);
        assert_eq!(app.stage, Stage::ComposeModal);
        dispatch(&mut app, key(KeyCode::Esc));
        assert!(!app.compose_picker_open, "picker dismissed");
        assert_eq!(app.stage, Stage::ComposeModal, "compose modal stays open");
    }

    #[test]
    fn send_routes_broadcast_through_mock_sender_via_picker() {
        // PR-UI-6 fixup (qa Gap 4): the broadcast path needs the
        // same MockMessageSender pin the DM path got in PR-UI-5.
        // Pins both per-channel-correct-id (picker selection
        // flows through to the send call) AND routes-through-
        // `broadcast()`-not-`send()` (no DM call recorded).
        use crate::compose::test_support::MockMessageSender;
        let sender = MockMessageSender::default();
        let mailbox = EmptyMailbox;
        let mut app = App::new();
        app.replace_team(fixture_team_with_channels(
            vec![agent("writing:manager", AgentState::Running)],
            vec![
                channel("writing:all", "writing"),
                channel("writing:editorial", "writing"),
                channel("writing:critique", "writing"),
            ],
        ));
        app.dismiss_splash();
        // Open picker, walk to channel index 1 (`editorial`),
        // confirm, type a body, Ctrl+Enter to send.
        super::handle_event(
            &mut app,
            key(KeyCode::Char('!')),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        super::handle_event(
            &mut app,
            key(KeyCode::Char('j')),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        super::handle_event(
            &mut app,
            key(KeyCode::Enter),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        for c in "ship docs".chars() {
            super::handle_event(
                &mut app,
                key(KeyCode::Char(c)),
                &NoopDecider,
                &sender,
                &mailbox,
            );
        }
        super::handle_event(
            &mut app,
            key_with(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            &NoopDecider,
            &sender,
            &mailbox,
        );
        let dm_calls = sender.dm_calls.lock().unwrap().clone();
        let bcast_calls = sender.broadcast_calls.lock().unwrap().clone();
        assert!(dm_calls.is_empty(), "broadcast must not route via send_dm");
        assert_eq!(bcast_calls.len(), 1);
        assert_eq!(
            bcast_calls[0].0, "writing:editorial",
            "channel id from picker selection"
        );
        assert_eq!(bcast_calls[0].1, "ship docs");
        assert_eq!(app.stage, Stage::Triptych, "modal closes on send");
    }

    #[test]
    fn bang_chord_opens_picker_when_channels_available() {
        let mut app = App::new();
        app.replace_team(fixture_team_with_channels(
            vec![agent("writing:manager", AgentState::Running)],
            vec![
                channel("writing:all", "writing"),
                channel("writing:editorial", "writing"),
                channel("writing:critique", "writing"),
            ],
        ));
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('!')));
        assert_eq!(app.stage, Stage::ComposeModal);
        assert!(app.compose_picker_open);
        // Walk the picker.
        dispatch(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.compose_picker_index, 1);
        // Confirm pulls into compose target.
        dispatch(&mut app, key(KeyCode::Enter));
        assert!(!app.compose_picker_open, "picker closes on confirm");
        match app.compose_target.as_ref() {
            Some(crate::compose::ComposeTarget::Broadcast { channel_id, .. }) => {
                assert_eq!(channel_id, "writing:editorial");
            }
            other => panic!("expected Broadcast target, got {other:?}"),
        }
    }

    #[test]
    fn mailbox_first_layout_seeds_channel_selection_on_entry() {
        let mut app = App::new();
        app.replace_team(fixture_team_with_channels(
            vec![agent("writing:manager", AgentState::Running)],
            vec![
                channel("writing:all", "writing"),
                channel("writing:editorial", "writing"),
            ],
        ));
        app.dismiss_splash();
        assert!(app.selected_channel.is_none());
        app.toggle_mailbox_first_layout();
        assert_eq!(app.selected_channel, Some(0));
    }

    #[test]
    fn help_overlay_opens_on_question_mark_closes_on_esc() {
        let mut app = App::new();
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('?')));
        assert_eq!(app.stage, Stage::HelpOverlay);
        dispatch(&mut app, key(KeyCode::Esc));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn tutorial_opens_on_t_advances_and_closes() {
        let mut app = App::new();
        app.dismiss_splash();
        dispatch(&mut app, key(KeyCode::Char('t')));
        assert_eq!(app.stage, Stage::Tutorial);
        assert_eq!(app.tutorial_step, 0);
        // Any non-Esc/back key advances.
        dispatch(&mut app, key(KeyCode::Char(' ')));
        assert_eq!(app.tutorial_step, 1);
        // `k` walks back.
        dispatch(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.tutorial_step, 0);
        // Esc closes from any step.
        dispatch(&mut app, key(KeyCode::Esc));
        assert_eq!(app.stage, Stage::Triptych);
    }

    #[test]
    fn tutorial_walk_back_at_step_zero_is_no_op() {
        // qa Gap C fold: pin the chosen behaviour for `k`/`Up`/`p`
        // at step 0 — saturating decrement keeps `tutorial_step`
        // at 0 rather than wrapping. Any future shift to
        // wrap-to-end would break this test, which is the point.
        let mut app = App::new();
        app.dismiss_splash();
        app.enter_tutorial();
        assert_eq!(app.tutorial_step, 0);
        dispatch(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.tutorial_step, 0, "step-0 walk-back is no-op");
        // The walk-back keypress must NOT close the tutorial
        // either — the Stage stays.
        assert_eq!(app.stage, Stage::Tutorial);
    }

    #[test]
    fn ctrl_pipe_adds_vertical_split_ctrl_minus_adds_horizontal() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        app.dismiss_splash();
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
        dispatch(
            &mut app,
            key_with(KeyCode::Char('-'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.detail_splits.len(), 2);
        assert_eq!(app.detail_splits[0].1, SplitOrientation::Vertical);
        assert_eq!(app.detail_splits[1].1, SplitOrientation::Horizontal);
    }

    #[test]
    fn ctrl_w_q_chord_prefix_closes_focused_split() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        app.dismiss_splash();
        // Two splits — `Ctrl+W` arms only when there's something
        // to close.
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
        dispatch(
            &mut app,
            key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
        );
        dispatch(
            &mut app,
            key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.pending_chord, Some(KeyCode::Char('w')));
        // Plain `q` (no modifier) is now interpreted as the
        // chord-prefix follow-up — close split, NOT quit.
        dispatch(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.detail_splits.len(), 1);
        assert_eq!(app.stage, Stage::Triptych, "must not enter quit confirm");
        assert_eq!(app.pending_chord, None, "chord cleared");
    }

    #[test]
    fn ctrl_w_o_chord_keeps_only_focused_split() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        app.dismiss_splash();
        for _ in 0..3 {
            dispatch(
                &mut app,
                key_with(KeyCode::Char('|'), KeyModifiers::CONTROL),
            );
        }
        // Focus the middle split.
        app.selected_split = 1;
        let kept_id = app.detail_splits[1].0.clone();
        dispatch(
            &mut app,
            key_with(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        dispatch(&mut app, key(KeyCode::Char('o')));
        assert_eq!(app.detail_splits.len(), 1);
        assert_eq!(app.detail_splits[0].0, kept_id);
        assert_eq!(app.selected_split, 0);
    }

    #[test]
    fn add_detail_split_saturates_at_four_with_explicit_4_and_5_calls() {
        // qa Gap 2 fold: pin the cap explicitly. Reaching exactly
        // 4 must stick; the 5th call must be a no-op (not panic,
        // not silently grow). If `add_detail_split` ever returns
        // a Result, this test catches the silent-success regression.
        let mut app = App::new();
        app.replace_team(fixture_team(vec![agent("p:a", AgentState::Running)]));
        for _ in 0..4 {
            app.add_detail_split();
        }
        assert_eq!(app.detail_splits.len(), 4);
        let snapshot_len = app.detail_splits.len();
        app.add_detail_split();
        assert_eq!(app.detail_splits.len(), snapshot_len, "5th call rejected");
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
