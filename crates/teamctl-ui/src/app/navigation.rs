//! Pane, roster, split, and mailbox navigation.

use super::{App, SplitOrientation, Stage, MAX_DETAIL_LINES, PANE_REFRESH_INTERVAL};
use crate::data::TeamSnapshot;
use crate::mailbox::MailboxInputKind;
use crate::triptych::MainLayout;
use std::time::Instant;

impl App {
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
        self.invalidate_detail();
    }
    pub fn toggle_mailbox_first_layout(&mut self) {
        self.layout = self.layout.toggle_mailbox_first();
        self.invalidate_detail();
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
        self.selected_split = self.detail_splits.len();
        self.invalidate_detail();
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
        let cell = self.selected_split.min(self.detail_splits.len());
        if cell == 0 {
            let id = self.detail_splits.remove(0).0;
            self.select_agent_id(&id);
        } else {
            self.detail_splits.remove(cell - 1);
        }
        self.selected_split = cell.saturating_sub(1).min(self.detail_splits.len());
        self.invalidate_detail();
    }
    pub fn cycle_split_next(&mut self) {
        if self.detail_splits.is_empty() {
            return;
        }
        self.selected_split = (self.selected_split + 1) % (self.detail_splits.len() + 1);
        self.invalidate_detail();
    }
    pub fn cycle_split_prev(&mut self) {
        if self.detail_splits.is_empty() {
            return;
        }
        self.selected_split = if self.selected_split == 0 {
            self.detail_splits.len()
        } else {
            self.selected_split - 1
        };
        self.invalidate_detail();
    }

    pub(super) fn close_other_splits(&mut self) {
        let id = self
            .detail_cell_agent(self.selected_split)
            .map(|agent| agent.id.clone());
        if let Some(id) = id {
            self.select_agent_id(&id);
        }
        self.detail_splits.clear();
        self.selected_split = 0;
        self.invalidate_detail();
    }

    fn select_agent_id(&mut self, id: &str) {
        let selected = self.team.agents.iter().position(|agent| agent.id == id);
        if self.selected_agent != selected {
            self.selected_agent = selected;
            self.mailbox.reset();
        }
    }

    pub(super) fn invalidate_detail(&mut self) {
        self.detail_buffer.clear();
        self.detail_buffer_activity = None;
        self.last_pane_refresh = Instant::now() - PANE_REFRESH_INTERVAL;
    }

    /// Cell zero is the roster selection; remaining cells are extra splits.
    pub fn detail_cell_agent(&self, cell: usize) -> Option<&crate::data::AgentInfo> {
        if cell == 0 {
            return self
                .selected_agent
                .and_then(|index| self.team.agents.get(index));
        }
        let id = &self.detail_splits.get(cell - 1)?.0;
        self.team.agents.iter().find(|agent| &agent.id == id)
    }

    pub fn cycle_mailbox_tab(&mut self) {
        self.mailbox_tab = self.mailbox_tab.next();
    }

    pub fn cycle_mailbox_tab_back(&mut self) {
        self.mailbox_tab = self.mailbox_tab.prev();
    }

    // T-131 PR-1: per-tab row cursor controls. Each delegates to the
    // matching `MailboxBuffers` method on the active tab, keeping the
    // App-level surface symmetric with the keybindings in
    // `handle_event` (Up/Down/j/k, PageUp/PageDown, Home/End).
    pub fn mailbox_cursor_down(&mut self) {
        self.mailbox.move_cursor_down(self.mailbox_tab);
    }

    pub fn mailbox_cursor_up(&mut self) {
        self.mailbox.move_cursor_up(self.mailbox_tab);
    }

    pub fn mailbox_page_down(&mut self) {
        self.mailbox.page_cursor_down(self.mailbox_tab);
    }

    pub fn mailbox_page_up(&mut self) {
        self.mailbox.page_cursor_up(self.mailbox_tab);
    }

    pub fn mailbox_cursor_home(&mut self) {
        self.mailbox.cursor_home(self.mailbox_tab);
    }

    pub fn mailbox_cursor_end(&mut self) {
        self.mailbox.cursor_end(self.mailbox_tab);
    }

    // T-131 PR-2: mailbox filter / search input mode. Singleton state
    // (only one input open at a time) drives editing into the active
    // tab's per-tab `filter_text` or `search_text` on MailboxBuffers.

    /// Open the sender-substring filter input on the active tab.
    /// Snapshots the current value so Esc can revert.
    pub fn open_mailbox_filter_input(&mut self) {
        self.mailbox_input_snapshot = self.mailbox.filter_text(self.mailbox_tab).to_string();
        self.mailbox_input_mode = Some(MailboxInputKind::Filter);
    }

    /// Open the body-substring search input on the active tab.
    /// Snapshots the current value so Esc can revert.
    pub fn open_mailbox_search_input(&mut self) {
        self.mailbox_input_snapshot = self.mailbox.search_text(self.mailbox_tab).to_string();
        self.mailbox_input_mode = Some(MailboxInputKind::Search);
    }

    /// Append `c` to the active input buffer. visible_indices
    /// recomputes live; the cursor re-clamps inside MailboxBuffers.
    pub fn mailbox_input_push_char(&mut self, c: char) {
        if let Some(kind) = self.mailbox_input_mode {
            self.mailbox.input_push_char(self.mailbox_tab, kind, c);
        }
    }

    /// Pop one character from the active input buffer.
    pub fn mailbox_input_pop_char(&mut self) {
        if let Some(kind) = self.mailbox_input_mode {
            self.mailbox.input_pop_char(self.mailbox_tab, kind);
        }
    }

    /// Confirm and close the input — keep the operator's typed text.
    pub fn mailbox_input_confirm(&mut self) {
        self.mailbox_input_mode = None;
        self.mailbox_input_snapshot.clear();
    }

    /// Cancel and close the input — revert the active buffer to the
    /// pre-open snapshot so the operator can back out without losing
    /// the prior filter / search.
    pub fn mailbox_input_cancel(&mut self) {
        if let Some(kind) = self.mailbox_input_mode {
            let snapshot = std::mem::take(&mut self.mailbox_input_snapshot);
            self.mailbox.set_input(self.mailbox_tab, kind, snapshot);
        }
        self.mailbox_input_mode = None;
        self.mailbox_input_snapshot.clear();
    }

    // T-131 PR-3: mailbox detail modal — snapshot-at-open, Esc/q
    // close, j/k scroll. The snapshot captures the row content at
    // open time so the rendered modal is stable across underlying
    // buffer drain (variant (a) locked).

    /// Open the detail modal on the currently-selected mailbox row.
    /// No-op when `visible_indices` is empty (no row to select) so
    /// `Enter` on an empty / fully-filtered tab silently does
    /// nothing rather than opening a modal on garbage.
    pub fn open_mailbox_detail_modal(&mut self) {
        let tab = self.mailbox_tab;
        let visible = self.mailbox.visible_indices(tab);
        if visible.is_empty() {
            return;
        }
        let idx = self.mailbox.cursor(tab).selected_idx.min(visible.len() - 1);
        let row_idx = visible[idx];
        let row = self.mailbox.rows(tab).get(row_idx).cloned();
        if let Some(row) = row {
            self.mailbox_detail_modal = Some(row);
            self.mailbox_detail_scroll = 0;
            self.stage = Stage::MailboxDetailModal;
        }
    }

    /// Close the detail modal and return to the Triptych. Clears
    /// the snapshot; the row cursor underneath is untouched.
    pub fn close_mailbox_detail_modal(&mut self) {
        self.mailbox_detail_modal = None;
        self.mailbox_detail_scroll = 0;
        self.stage = Stage::Triptych;
    }

    /// Scroll the detail modal body one wrapped line down. Caller
    /// supplies the maximum scroll value (lines beyond which there
    /// is no content); we clamp.
    pub fn mailbox_detail_scroll_down(&mut self) {
        // The renderer enforces the upper bound at draw time when it
        // knows the wrapped-body height; this helper just bumps the
        // offset. Saturating add caps at u16::MAX which is far
        // beyond any realistic body length.
        self.mailbox_detail_scroll = self.mailbox_detail_scroll.saturating_add(1);
    }

    /// Scroll the detail modal body one wrapped line up.
    pub fn mailbox_detail_scroll_up(&mut self) {
        self.mailbox_detail_scroll = self.mailbox_detail_scroll.saturating_sub(1);
    }

    pub fn cycle_focus_back(&mut self) {
        self.focused_pane = self.focused_pane.prev();
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
            self.invalidate_detail();
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
            self.invalidate_detail();
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
        let prior_target = self.stream_target_session();
        let prior_channel = self
            .selected_channel
            .and_then(|i| self.team.channels.get(i))
            .map(|ch| ch.id.clone());
        let had_channel_selection = self.selected_channel.is_some();
        self.mailbox_db_path = crate::data::broker_path(&team.root)
            .unwrap_or_else(|_| team.root.join("state/mailbox.db"));
        self.team = team;
        self.selected_agent = match (prior_id.clone(), self.team.agents.is_empty()) {
            (_, true) => None,
            (Some(id), false) => self.team.agents.iter().position(|a| a.id == id).or(Some(0)),
            (None, false) => Some(0),
        };
        if prior_id != self.selected_agent_id() {
            self.mailbox.reset();
            self.invalidate_detail();
        }
        // Preserve the focused cell's original entry, including duplicate
        // agent ids. Removing a preceding cell must never retarget input.
        let selected_entry = self.selected_split.checked_sub(1);
        let mut old_index = 0;
        let mut kept = 0;
        let mut selected = None;
        self.detail_splits.retain(|(id, _)| {
            let keep = self.team.agents.iter().any(|a| &a.id == id);
            if keep {
                kept += 1;
                if selected_entry == Some(old_index) {
                    selected = Some(kept);
                }
            }
            old_index += 1;
            keep
        });
        self.selected_split = selected.unwrap_or(0);
        self.selected_channel = prior_channel
            .and_then(|id| self.team.channels.iter().position(|ch| ch.id == id))
            .or_else(|| (had_channel_selection && !self.team.channels.is_empty()).then_some(0));
        self.compose_picker_index = self
            .compose_picker_index
            .min(self.team.channels.len().saturating_sub(1));
        if prior_target != self.stream_target_session() {
            self.invalidate_detail();
            if self.stage == Stage::StreamKeys {
                self.stage = Stage::Triptych;
            }
        }
    }

    /// Return the focused agent's tmux session name, if any. Used
    /// by the run loop to know which session to capture.
    pub fn focused_session(&self) -> Option<&str> {
        self.selected_agent
            .and_then(|i| self.team.agents.get(i))
            .map(|a| a.tmux_session.as_str())
    }

    /// Tmux session that stream-keys mode should target. Cell 0 of
    /// the detail-pane split layout is always the focused agent;
    /// cells 1..N are the entries in `detail_splits`. When the
    /// operator has focused a non-zero split, route stream-keys to
    /// that split's agent — that's the cell visually showing as the
    /// focus ring, so it's the one the operator expects to type into.
    pub fn stream_target_session(&self) -> Option<String> {
        let cell = if self.layout == MainLayout::Triptych {
            self.selected_split
        } else {
            0
        };
        self.detail_cell_agent(cell).map(|a| a.tmux_session.clone())
    }

    /// Enter stream-keys mode. No-op unless an agent is selected —
    /// without a target session there's nothing to forward to.
    /// Caller is responsible for the focused-pane gate (entry chord
    /// only fires from `focused_pane == Pane::Detail`).
    pub fn enter_stream_keys(&mut self) {
        if self.stream_target_session().is_none() {
            return;
        }
        self.previous_stage = self.stage;
        self.stage = Stage::StreamKeys;
    }

    /// Exit stream-keys mode and return to whichever stage opened it.
    /// Ctrl+E toggles the mode; Esc and Ctrl+C reach the agent.
    pub fn exit_stream_keys(&mut self) {
        self.stage = self.previous_stage;
    }

    /// Replace the detail buffer, clipped at the recent-line cap.
    pub fn set_detail_buffer(&mut self, lines: Vec<String>) {
        let len = lines.len();
        let start = len.saturating_sub(MAX_DETAIL_LINES);
        self.detail_buffer = lines[start..].to_vec();
    }
}
