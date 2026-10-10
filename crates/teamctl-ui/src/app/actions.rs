//! Compose and approval workflows.

use super::{refresh_mailbox, App, Stage};
use crate::approvals::{Approval, ApprovalDecider, Decision};
use crate::compose::{ComposeTarget, Editor, MessageSender};
use crate::mailbox::MailboxSource;

impl App {
    /// Open the channel picker before editing a broadcast draft.
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

    /// T-32: open the path-input overlay. Resets the buffer so a
    /// previously-cancelled draft can't carry over.
    pub fn open_compose_attach_input(&mut self) {
        self.compose_attach_input_open = true;
        self.compose_attach_buffer.clear();
    }

    /// T-32: append a `📎 attachment: <path>` line to the compose
    /// editor and close the overlay. The line lands as a fresh row
    /// at the end of the body so the operator can edit it (or delete
    /// it) before sending. Whitespace-only buffers are ignored — Tab
    /// followed by Enter shouldn't insert an empty marker.
    pub fn confirm_compose_attach_input(&mut self) {
        let path = self.compose_attach_buffer.trim().to_string();
        if !path.is_empty() {
            let marker = format!("📎 attachment: {path}");
            // The body's final-trailing-blank rule (Editor::body)
            // strips empty trailing lines, so an empty last line
            // doesn't matter — we always push the marker as a new
            // line after current contents.
            if let Some(last) = self.compose_editor.lines.last_mut() {
                if !last.is_empty() {
                    self.compose_editor.lines.push(marker);
                } else {
                    *last = marker;
                }
            } else {
                self.compose_editor.lines.push(marker);
            }
            // Park the cursor at end of the new line so subsequent
            // typing in Insert mode picks up after the marker.
            self.compose_editor.cursor_row = self.compose_editor.lines.len() - 1;
            self.compose_editor.cursor_col = self
                .compose_editor
                .lines
                .last()
                .map(|l| l.len())
                .unwrap_or(0);
        }
        self.close_compose_attach_input();
    }

    pub fn close_compose_attach_input(&mut self) {
        self.compose_attach_input_open = false;
        self.compose_attach_buffer.clear();
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
    /// otherwise and preserves the selected approval by id. If that
    /// approval was resolved out-of-band, clamps to a surviving row.
    pub fn replace_approvals(&mut self, approvals: Vec<Approval>) {
        let selected_id = self.focused_approval().map(|approval| approval.id);
        self.pending_approvals = approvals;
        if let Some(index) =
            selected_id.and_then(|id| self.pending_approvals.iter().position(|a| a.id == id))
        {
            self.selected_approval = index;
        }
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
        // T-32: ensure the attach overlay state can't survive a
        // close-and-reopen of the modal.
        self.compose_attach_input_open = false;
        self.compose_attach_buffer.clear();
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
}
