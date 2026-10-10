//! Application state and construction.

use super::REFRESH_INTERVAL;
use crate::approvals::Approval;
use crate::compose::{ComposeTarget, Editor};
use crate::data::TeamSnapshot;
use crate::mailbox::{MailboxBuffers, MailboxInputKind, MailboxTab, MessageRow};
use crate::theme::{detect_capabilities, Capabilities};
use crate::triptych::{MainLayout, Pane};
use crate::tutorial;
use crossterm::event::KeyCode;
use std::time::Instant;

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
    /// Stream-keys mode (T-108). Activated by `Ctrl+E` while the
    /// detail pane is focused; every subsequent keystroke (except
    /// `Esc`, the exit chord) is forwarded to the focused agent's
    /// tmux pane via `tmux send-keys`. Ctrl+E exits. The Triptych keeps rendering
    /// underneath — the 1s refresh tick still captures whatever the
    /// agent prints in response — so the operator interacts with
    /// the agent in real time without leaving the UI.
    StreamKeys,
    /// Mailbox detail modal (T-131 PR-3). `Enter` on a selected
    /// mailbox row snapshots that row into `mailbox_detail_modal`
    /// and flips here; the modal renders the full message body
    /// (wrapped, j/k-scrollable) plus sender / recipient / kind /
    /// absolute timestamp / transport / message id. The snapshot is
    /// captured AT open-time so the rendered content is stable
    /// across any subsequent underlying-buffer drain (PR-3 variant
    /// (a) locked: snapshot-at-open, not id-tracking — resolves the
    /// PR-1 kian-#1 identity question at the point it actually
    /// matters). `Esc` or `q` closes.
    MailboxDetailModal,
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
    /// Broker path resolved from the loaded compose configuration.
    pub mailbox_db_path: std::path::PathBuf,
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
    /// Last time the focused agent's pane was re-captured on the fast
    /// `PANE_REFRESH_INTERVAL` cadence (between full refreshes).
    pub last_pane_refresh: Instant,
    pub running: bool,
    /// First-launch detection — when the marker file exists, future
    /// stacked-PRs (PR-UI-7) skip the tutorial after splash. PR-UI-1
    /// only reads the flag; nothing routes off it yet.
    pub tutorial_completed: bool,
    /// Active tab inside the mailbox pane (PR-UI-3). Walked with
    /// `←` / `→` when `focused_pane == Mailbox` (T-124 hard-swapped
    /// the prior `[` / `]` chord for arrow keys; T-074 bug 6 is
    /// the gating-on-focus invariant). `Tab` always cycles pane
    /// focus, never mailbox tabs — the previous "Tab cycles tabs
    /// when mailbox is focused" shape stranded operators inside
    /// the mailbox.
    pub mailbox_tab: MailboxTab,
    /// Per-tab buffers + cursors for the focused agent's mailbox
    /// view. Reset whenever the focused agent changes — switching
    /// agents starts the operator at the head of fresh traffic.
    pub mailbox: MailboxBuffers,
    /// T-131 PR-2: which mailbox input the operator is currently
    /// editing, if any. Singleton — only one input open at a time
    /// across all tabs. When `Some`, `Pane::Mailbox` keystrokes route
    /// to the per-tab `filter_text` / `search_text` buffer on
    /// [`MailboxBuffers`] (the data lives there, per-tab; this is
    /// just the editing-UI flag).
    pub mailbox_input_mode: Option<MailboxInputKind>,
    /// Pre-open snapshot of the active input buffer — restored on
    /// `Esc` (cancel-revert) so the operator can back out without
    /// losing the prior filter/search. Empty between sessions.
    pub mailbox_input_snapshot: String,
    /// T-131 PR-3: mailbox detail modal — the row content the
    /// operator opened. Captured AT open-time and rendered from
    /// here independent of the underlying mailbox buffer: any
    /// subsequent `extend()` drain that would shift indices on the
    /// row cursor leaves this snapshot intact, so the operator
    /// sees the message they clicked, not whatever now happens to
    /// sit at the same index (variant (a) locked). `None` when no
    /// modal is open.
    pub mailbox_detail_modal: Option<MessageRow>,
    /// T-131 PR-3: vertical scroll offset (in wrapped body lines)
    /// within an open detail modal. Reset to 0 when the modal
    /// opens; bumped by `j` / `Down` / `k` / `Up` while the modal
    /// is the active stage. Ignored when no modal is open.
    pub mailbox_detail_scroll: u16,
    /// T-131 PR-4: wall-clock seconds at the last render tick. The
    /// mailbox-row relative-time indicator (`2m` / `1h` / `3d`)
    /// reads from here so render is a pure function of `App` —
    /// snapshot tests can pin time deterministically by setting
    /// this field (otherwise wall-clock would diff snapshots every
    /// run). The `run` loop refreshes this before each
    /// `terminal.draw`; defaults to 0 in `App::new` so a freshly
    /// constructed test app + sent_at=0 fixture rows render `now`
    /// stably.
    pub now_secs: f64,
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
    /// T-32: when `true`, the compose modal renders a single-line
    /// path-input overlay instead of the editor; Enter appends a
    /// `📎 attachment: <path>` line to the editor body and closes the
    /// overlay. Tab inside the editor opens it; Esc inside the
    /// overlay cancels back to the editor (matches the picker
    /// overlay's modal-vs-modal symmetry from PR-UI-6).
    pub compose_attach_input_open: bool,
    /// Single-line buffer for the path-input overlay. Reset on close
    /// so a cancelled draft can't leak into the next attach attempt.
    pub compose_attach_buffer: String,
    /// T-199: per-session cache of the last Detail-pane size we
    /// pushed to `tmux resize-window`. The run loop diffs the current
    /// Detail rect against this on every frame and only spawns the
    /// tmux command when the size actually changed — common case
    /// (no resize, no focus switch) is a HashMap lookup. Keyed by
    /// `tmux_session` (e.g. `t-hello-manager`). See
    /// `crate::pane_resize`.
    pub last_synced_pane_sizes: std::collections::HashMap<String, (u16, u16)>,
    /// Failed resize attempts are retried after a bounded delay.
    pub pane_resize_failures: std::collections::HashMap<String, ((u16, u16), Instant)>,
    /// #277: the `(session, window_activity ts)` of the content currently
    /// in `detail_buffer`, as last captured by the fast-cadence path. The
    /// fast path skips the heavy `capture-pane` only when the focused
    /// session AND its activity ts both still match — so a focus switch
    /// (different session) or new output (changed ts) always re-captures,
    /// and a stale frame can't survive on screen. `None` forces a capture.
    /// The slow 1 Hz refreshes re-capture unconditionally (the safety net)
    /// and may leave this lagging by one tick: at most one redundant fast
    /// capture, never a wrong frame.
    pub detail_buffer_activity: Option<(String, u64)>,
    /// T-209: live system handle for the bottom status bar's
    /// CPU% + RAM% indicator. Refreshed in-place on the existing
    /// 1-second App tick (see `refresh_with_default_sources` and the
    /// run-loop tick at the top of `run()`); no background thread.
    /// `default-features = false` + only the `system` feature is
    /// enabled in the dep to keep the compile surface narrow. See
    /// `crate::status_bar`.
    pub sysinfo: sysinfo::System,
    /// T-212 / #431 rate-limit indicator gate. `true` by default; `false`
    /// only when `TEAMCTL_UI_RATE_LIMIT_INDICATOR=0` was set at App::new().
    /// The bottom status bar's center slot only renders when this is true.
    /// Tests can flip the field directly to exercise both branches without
    /// process-wide env-var racing.
    pub rate_limit_indicator_enabled: bool,
}

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
            mailbox_db_path: std::path::PathBuf::from("state/mailbox.db"),
            selected_agent: None,
            detail_buffer: Vec::new(),
            version: env!("CARGO_PKG_VERSION"),
            capabilities: detect_capabilities(),
            splash_started: Instant::now(),
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            last_pane_refresh: Instant::now(),
            running: true,
            tutorial_completed: tutorial::is_completed(),
            mailbox_tab: MailboxTab::Inbox,
            mailbox: MailboxBuffers::default(),
            mailbox_input_mode: None,
            mailbox_input_snapshot: String::new(),
            mailbox_detail_modal: None,
            mailbox_detail_scroll: 0,
            now_secs: 0.0,
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
            compose_attach_input_open: false,
            compose_attach_buffer: String::new(),
            pending_chord: None,
            tutorial_pending_for_team: false,
            spinner_frame: 0,
            tutorial_step: 0,
            last_synced_pane_sizes: std::collections::HashMap::new(),
            pane_resize_failures: std::collections::HashMap::new(),
            detail_buffer_activity: None,
            // sysinfo's `new()` allocates but doesn't read any metrics;
            // the first values are populated by the first refresh tick
            // in `refresh_with_default_sources`. Until then the status
            // bar reads zeros — operator sees the bar shape but the
            // numbers stabilize after ~1 second.
            sysinfo: sysinfo::System::new(),
            // T-212 / #431: the per-agent rate-limit indicator is on by
            // default now that the #431 hook feeds it hit rows. Operators
            // opt OUT by setting `TEAMCTL_UI_RATE_LIMIT_INDICATOR=0`; any
            // other value (or the var unset) leaves it enabled. Read once at
            // App::new(); flipping the var mid-session requires a TUI restart.
            rate_limit_indicator_enabled: std::env::var_os("TEAMCTL_UI_RATE_LIMIT_INDICATOR")
                .is_none_or(|v| v != "0"),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
