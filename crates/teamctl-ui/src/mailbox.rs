//! Mailbox-pane data source and tab definitions.
//!
//! Four filter shapes, one per tab in SPEC §2's Triptych mailbox:
//!
//! - `Inbox` — DMs whose `recipient = '<project>:<agent>'`.
//! - `Sent` — every row whose `sender = '<project>:<agent>'`,
//!   irrespective of recipient class. Closes the "did this agent
//!   actually emit X" debug loop without pivoting to the recipient.
//! - `Channel` — channel traffic for channels the focused agent is
//!   a member of (recipient is `'channel:<channel_id>'`, filtered
//!   through `channel_members`).
//! - `Wire` — project-wide broadcast traffic on the `all` channel
//!   (`recipient = 'channel:<project>:all'`).
//!
//! INVARIANT: every `messages.recipient` value falls into exactly
//! one of three prefix classes — `<project>:<agent>` (DM, no scheme
//! prefix; the channel-or-user split below depends on this absence),
//! `channel:<channel_id>`, or `user:<handle>`. `data::mailbox_counts`
//! relies on the same contract when it filters out channel/user rows
//! for the per-agent unread-mail counter; if a fourth prefix class
//! ever lands, the comment there and the queries here both need to
//! learn it. Sent is the one tab whose filter is sender-side and
//! recipient-class-agnostic — it returns rows from all three
//! recipient prefix classes.

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxTab {
    Inbox,
    Sent,
    Channel,
    Wire,
}

impl MailboxTab {
    pub const ALL: [MailboxTab; 4] = [
        MailboxTab::Inbox,
        MailboxTab::Sent,
        MailboxTab::Channel,
        MailboxTab::Wire,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MailboxTab::Inbox => "Inbox",
            MailboxTab::Sent => "Sent",
            MailboxTab::Channel => "Channel",
            MailboxTab::Wire => "Wire",
        }
    }

    pub fn empty_hint(self) -> &'static str {
        match self {
            MailboxTab::Inbox => "(no DMs)",
            MailboxTab::Sent => "(no sent messages)",
            MailboxTab::Channel => "(no channel traffic)",
            MailboxTab::Wire => "(quiet)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            MailboxTab::Inbox => MailboxTab::Sent,
            MailboxTab::Sent => MailboxTab::Channel,
            MailboxTab::Channel => MailboxTab::Wire,
            MailboxTab::Wire => MailboxTab::Inbox,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            MailboxTab::Inbox => MailboxTab::Wire,
            MailboxTab::Sent => MailboxTab::Inbox,
            MailboxTab::Channel => MailboxTab::Sent,
            MailboxTab::Wire => MailboxTab::Channel,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub sender: String,
    pub recipient: String,
    pub text: String,
    pub sent_at: f64,
}

/// Format a single row for the mailbox pane. Kept terse: prefix in
/// brackets + one-line body. Multi-line bodies are flattened with a
/// space so a single message stays one row in the pane.
///
/// Prefix is tab-aware (T-231):
///
/// - **Inbox / Channel / Wire** → `[<senderName>]`. Sender is the
///   useful disambiguator for received rows; resolved via
///   [`crate::data::agent_label`] so `display_name` carries when
///   set.
/// - **Sent** → `[→<recipientName>]`. Sender on a Sent row is
///   always the focused agent (that's the filter), so showing it is
///   redundant. Operators want to see WHO the agent talked to;
///   recipient resolution goes through
///   [`crate::data::recipient_label`] which handles agent,
///   `channel:`, and `user:` recipient shapes.
pub fn render_row(row: &MessageRow, team: &crate::data::TeamSnapshot, tab: MailboxTab) -> String {
    let one_line: String = row
        .text
        .replace('\n', " ")
        .replace('\r', "")
        .chars()
        .take(180)
        .collect();
    match tab {
        MailboxTab::Sent => {
            let recipient = crate::data::recipient_label(team, &row.recipient);
            format!("[→{recipient}] {one_line}")
        }
        MailboxTab::Inbox | MailboxTab::Wire => {
            let sender = crate::data::agent_label(team, &row.sender);
            format!("[{sender}] {one_line}")
        }
        MailboxTab::Channel => {
            // T-249: the Channel tab folds every subscribed channel
            // into a single feed; without the channel name, operators
            // can't tell `#all` from `#dev` from `#docs`. Two
            // bracketed segments — channel, then sender — matching
            // the disambiguator-first convention T-231 set on Sent.
            // `recipient_label` already maps `channel:<p>:<n>` to
            // `#<n>`, so the resolution lives in one place.
            let channel = crate::data::recipient_label(team, &row.recipient);
            let sender = crate::data::agent_label(team, &row.sender);
            format!("[{channel}] [{sender}] {one_line}")
        }
    }
}

/// Lookup contract: each method returns rows newer than `after_id`
/// for the given filter, in ascending id order. Callers fold the
/// returned rows into a per-tab buffer and bump `after_id` to the
/// last returned id.
pub trait MailboxSource: Send + Sync {
    fn inbox(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>>;
    fn sent(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>>;
    fn channel_feed(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>>;
    fn wire(&self, project_id: &str, after_id: i64) -> Result<Vec<MessageRow>>;
}

/// Production impl reading the broker SQLite at `<root>/state/mailbox.db`.
/// Each call opens a fresh connection — `mailbox.db` is local and
/// short-lived connections cost effectively zero.
#[derive(Debug, Clone)]
pub struct BrokerMailboxSource {
    pub db_path: PathBuf,
}

impl BrokerMailboxSource {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn open(&self) -> Result<Option<Connection>> {
        if !self.db_path.is_file() {
            return Ok(None);
        }
        let conn = Connection::open(&self.db_path)?;
        Ok(Some(conn))
    }
}

impl MailboxSource for BrokerMailboxSource {
    fn inbox(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text, sent_at FROM messages
             WHERE id > ?1 AND recipient = ?2
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![after_id, agent_id], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    sender: r.get(1)?,
                    recipient: r.get(2)?,
                    text: r.get(3)?,
                    sent_at: r.get(4)?,
                })
            })?
            .flatten()
            .collect();
        Ok(rows)
    }

    fn sent(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        // Sender-side filter — every row the focused agent emitted,
        // irrespective of recipient class. Returns DMs, telegram
        // replies, channel posts, and wire broadcasts in a single
        // stream.
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text, sent_at FROM messages
             WHERE id > ?1 AND sender = ?2
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![after_id, agent_id], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    sender: r.get(1)?,
                    recipient: r.get(2)?,
                    text: r.get(3)?,
                    sent_at: r.get(4)?,
                })
            })?
            .flatten()
            .collect();
        Ok(rows)
    }

    fn channel_feed(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        // Same shape as `teamctl tail <agent>`'s channel arm: rows
        // whose recipient is a `channel:` URL the agent is a member
        // of. Membership lives in `channel_members.agent_id =
        // <project>:<agent>`.
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text, sent_at FROM messages
             WHERE id > ?1
               AND recipient IN (
                   SELECT 'channel:' || cm.channel_id FROM channel_members cm
                   WHERE cm.agent_id = ?2
               )
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![after_id, agent_id], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    sender: r.get(1)?,
                    recipient: r.get(2)?,
                    text: r.get(3)?,
                    sent_at: r.get(4)?,
                })
            })?
            .flatten()
            .collect();
        Ok(rows)
    }

    fn wire(&self, project_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        // The project-wide `all` channel is the broadcast wire.
        // Channel ids are `<project>:<name>`; messages address them
        // via `channel:<channel_id>`.
        let target = format!("channel:{project_id}:all");
        let mut stmt = conn.prepare(
            "SELECT id, sender, recipient, text, sent_at FROM messages
             WHERE id > ?1 AND recipient = ?2
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![after_id, target], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    sender: r.get(1)?,
                    recipient: r.get(2)?,
                    text: r.get(3)?,
                    sent_at: r.get(4)?,
                })
            })?
            .flatten()
            .collect();
        Ok(rows)
    }
}

/// Per-agent buffer state — four tabs, four `after_id` cursors.
/// Lives on `App` so swapping the focused agent resets the cursors
/// without trying to back-fill: the operator sees only forward
/// motion in the tab they're watching.
#[derive(Debug, Default, Clone)]
pub struct MailboxBuffers {
    pub inbox: Vec<MessageRow>,
    pub sent: Vec<MessageRow>,
    pub channel: Vec<MessageRow>,
    pub wire: Vec<MessageRow>,
    pub inbox_after: i64,
    pub sent_after: i64,
    pub channel_after: i64,
    pub wire_after: i64,
    // T-131 PR-1: UI cursor state per tab. `selected_idx` is an index
    // INTO `visible_indices(tab)`, not directly into `rows(tab)` — the
    // two coincide in PR-1 but PR-2 (filter+search) makes them diverge
    // without changing this invariant or any call site.
    pub inbox_cursor: CursorState,
    pub sent_cursor: CursorState,
    pub channel_cursor: CursorState,
    pub wire_cursor: CursorState,
}

/// UI cursor state for one mailbox tab. PR-1 stores only the selected
/// row index; the rendered scroll-window is derived at render time
/// from `selected_idx` + the actual pane height, so a terminal resize
/// just changes the next-paint window without touching persisted
/// state. `selected_idx` is an index into
/// [`MailboxBuffers::visible_indices`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub selected_idx: usize,
}

const MAX_TAB_ROWS: usize = 500;

/// PageUp/PageDown jump size — a screen-ish chunk of rows. Fixed for
/// PR-1 to keep scope surgical; a follow-up can wire this to the
/// actual rendered mailbox-pane height once that's plumbed onto App.
pub const PAGE_JUMP: usize = 10;

impl MailboxBuffers {
    pub fn rows(&self, tab: MailboxTab) -> &[MessageRow] {
        match tab {
            MailboxTab::Inbox => &self.inbox,
            MailboxTab::Sent => &self.sent,
            MailboxTab::Channel => &self.channel,
            MailboxTab::Wire => &self.wire,
        }
    }

    /// Indices into `rows(tab)` for the rows currently presented to
    /// the operator. PR-1 returns identity (every row visible); the
    /// abstraction exists so PR-2 (filter+search) can swap the body
    /// without touching cursor methods or the render call sites — the
    /// `selected_idx` invariant "index into visible_indices(tab)" is
    /// what makes that swap free.
    pub fn visible_indices(&self, tab: MailboxTab) -> Vec<usize> {
        (0..self.rows(tab).len()).collect()
    }

    pub fn cursor(&self, tab: MailboxTab) -> &CursorState {
        match tab {
            MailboxTab::Inbox => &self.inbox_cursor,
            MailboxTab::Sent => &self.sent_cursor,
            MailboxTab::Channel => &self.channel_cursor,
            MailboxTab::Wire => &self.wire_cursor,
        }
    }

    fn cursor_mut(&mut self, tab: MailboxTab) -> &mut CursorState {
        match tab {
            MailboxTab::Inbox => &mut self.inbox_cursor,
            MailboxTab::Sent => &mut self.sent_cursor,
            MailboxTab::Channel => &mut self.channel_cursor,
            MailboxTab::Wire => &mut self.wire_cursor,
        }
    }

    /// Move the cursor one row toward the tail; clamps at the last
    /// visible row (vim-like — no wrap).
    pub fn move_cursor_down(&mut self, tab: MailboxTab) {
        let max = self.visible_indices(tab).len().saturating_sub(1);
        let c = self.cursor_mut(tab);
        c.selected_idx = (c.selected_idx + 1).min(max);
    }

    /// Move the cursor one row toward the head; clamps at 0.
    pub fn move_cursor_up(&mut self, tab: MailboxTab) {
        let c = self.cursor_mut(tab);
        c.selected_idx = c.selected_idx.saturating_sub(1);
    }

    /// Jump a screen toward the tail.
    pub fn page_cursor_down(&mut self, tab: MailboxTab) {
        let max = self.visible_indices(tab).len().saturating_sub(1);
        let c = self.cursor_mut(tab);
        c.selected_idx = (c.selected_idx + PAGE_JUMP).min(max);
    }

    /// Jump a screen toward the head.
    pub fn page_cursor_up(&mut self, tab: MailboxTab) {
        let c = self.cursor_mut(tab);
        c.selected_idx = c.selected_idx.saturating_sub(PAGE_JUMP);
    }

    /// Jump to the first visible row.
    pub fn cursor_home(&mut self, tab: MailboxTab) {
        self.cursor_mut(tab).selected_idx = 0;
    }

    /// Jump to the last visible row.
    pub fn cursor_end(&mut self, tab: MailboxTab) {
        let max = self.visible_indices(tab).len().saturating_sub(1);
        self.cursor_mut(tab).selected_idx = max;
    }

    /// Fold a freshly-fetched batch into the appropriate tab,
    /// trimming to the last `MAX_TAB_ROWS`. Bumps the broker
    /// pagination cursor to the last returned id when the batch is
    /// non-empty. T-131 PR-1: when the UI cursor was already at the
    /// tail (or the tab was empty), follow new arrivals — matching
    /// the pre-T-131 "tail to whatever fits" UX. Always re-clamps the
    /// UI cursor against the (possibly drained) post-extend visible
    /// length so a stale index can never reference a missing row.
    pub fn extend(&mut self, tab: MailboxTab, batch: Vec<MessageRow>) {
        let prev_visible_len = self.visible_indices(tab).len();
        let was_at_tail =
            prev_visible_len == 0 || self.cursor(tab).selected_idx + 1 >= prev_visible_len;
        let last_id = batch.last().map(|r| r.id);
        let (buf, after) = match tab {
            MailboxTab::Inbox => (&mut self.inbox, &mut self.inbox_after),
            MailboxTab::Sent => (&mut self.sent, &mut self.sent_after),
            MailboxTab::Channel => (&mut self.channel, &mut self.channel_after),
            MailboxTab::Wire => (&mut self.wire, &mut self.wire_after),
        };
        buf.extend(batch);
        if buf.len() > MAX_TAB_ROWS {
            let drop = buf.len() - MAX_TAB_ROWS;
            buf.drain(..drop);
        }
        if let Some(id) = last_id {
            *after = id;
        }
        let new_visible_len = self.visible_indices(tab).len();
        let cur = self.cursor_mut(tab);
        if was_at_tail && new_visible_len > 0 {
            cur.selected_idx = new_visible_len - 1;
        } else if new_visible_len > 0 {
            let max = new_visible_len - 1;
            if cur.selected_idx > max {
                cur.selected_idx = max;
            }
        } else {
            cur.selected_idx = 0;
        }
    }

    /// Reset every tab's contents and cursor. Called when the
    /// focused agent changes — the new agent's `inbox` filter would
    /// otherwise skip historical rows that landed before our last
    /// `inbox_after`, and the UI cursor would point into the wrong
    /// agent's buffer.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub mod test_support {
    //! Shared mock — public so unit tests, integration tests, and
    //! downstream coverage can wire in a recorder without rolling
    //! their own. Matches the shape used by `compose::test_support`
    //! and `approvals::test_support`.

    use super::*;
    use std::sync::Mutex;

    /// Test stub — returns canned rows on each call, records every
    /// arg pair. Mailbox is the most-asserted test surface in
    /// PR-UI-3 so the recorder lets snapshot + interaction tests
    /// verify "is the right filter being asked the right thing."
    #[derive(Default)]
    pub struct MockMailboxSource {
        pub inbox_rows: Vec<MessageRow>,
        pub sent_rows: Vec<MessageRow>,
        pub channel_rows: Vec<MessageRow>,
        pub wire_rows: Vec<MessageRow>,
        pub inbox_calls: Mutex<Vec<(String, i64)>>,
        pub sent_calls: Mutex<Vec<(String, i64)>>,
        pub channel_calls: Mutex<Vec<(String, i64)>>,
        pub wire_calls: Mutex<Vec<(String, i64)>>,
    }

    impl MailboxSource for MockMailboxSource {
        fn inbox(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
            self.inbox_calls
                .lock()
                .unwrap()
                .push((agent_id.into(), after_id));
            Ok(self.inbox_rows.clone())
        }

        fn sent(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
            self.sent_calls
                .lock()
                .unwrap()
                .push((agent_id.into(), after_id));
            Ok(self.sent_rows.clone())
        }

        fn channel_feed(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
            self.channel_calls
                .lock()
                .unwrap()
                .push((agent_id.into(), after_id));
            Ok(self.channel_rows.clone())
        }

        fn wire(&self, project_id: &str, after_id: i64) -> Result<Vec<MessageRow>> {
            self.wire_calls
                .lock()
                .unwrap()
                .push((project_id.into(), after_id));
            Ok(self.wire_rows.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn row(id: i64, sender: &str, recipient: &str, text: &str) -> MessageRow {
        MessageRow {
            id,
            sender: sender.into(),
            recipient: recipient.into(),
            text: text.into(),
            sent_at: 0.0,
        }
    }

    #[test]
    fn next_cycles_inbox_sent_channel_wire_inbox() {
        let mut t = MailboxTab::Inbox;
        t = t.next();
        assert_eq!(t, MailboxTab::Sent);
        t = t.next();
        assert_eq!(t, MailboxTab::Channel);
        t = t.next();
        assert_eq!(t, MailboxTab::Wire);
        t = t.next();
        assert_eq!(t, MailboxTab::Inbox);
    }

    #[test]
    fn prev_cycles_inbox_wire_channel_sent_inbox() {
        let mut t = MailboxTab::Inbox;
        t = t.prev();
        assert_eq!(t, MailboxTab::Wire);
        t = t.prev();
        assert_eq!(t, MailboxTab::Channel);
        t = t.prev();
        assert_eq!(t, MailboxTab::Sent);
        t = t.prev();
        assert_eq!(t, MailboxTab::Inbox);
    }

    #[test]
    fn extend_appends_and_bumps_cursor() {
        let mut buf = MailboxBuffers::default();
        buf.extend(
            MailboxTab::Inbox,
            vec![row(7, "p:m", "p:dev", "hi"), row(8, "p:m", "p:dev", "yo")],
        );
        assert_eq!(buf.inbox.len(), 2);
        assert_eq!(buf.inbox_after, 8);
        // Empty batch must not move the cursor backward.
        buf.extend(MailboxTab::Inbox, vec![]);
        assert_eq!(buf.inbox_after, 8);
    }

    #[test]
    fn extend_trims_to_cap() {
        let mut buf = MailboxBuffers::default();
        let batch: Vec<MessageRow> = (1..=600).map(|i| row(i, "p:m", "p:dev", "x")).collect();
        buf.extend(MailboxTab::Wire, batch);
        assert_eq!(buf.wire.len(), MAX_TAB_ROWS);
        // Cap keeps the *latest* rows — the cursor reflects the
        // batch's actual high-water id, not the trimmed buffer's
        // first row.
        assert_eq!(buf.wire_after, 600);
        assert_eq!(buf.wire.last().unwrap().id, 600);
    }

    #[test]
    fn reset_clears_buffers_and_cursors() {
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, vec![row(3, "a", "b", "x")]);
        buf.extend(MailboxTab::Channel, vec![row(4, "a", "channel:p:all", "y")]);
        buf.reset();
        assert!(buf.inbox.is_empty());
        assert!(buf.channel.is_empty());
        assert_eq!(buf.inbox_after, 0);
        assert_eq!(buf.channel_after, 0);
    }

    fn empty_team() -> crate::data::TeamSnapshot {
        crate::data::TeamSnapshot::empty(std::path::PathBuf::from("/tmp"))
    }

    #[test]
    fn render_row_flattens_newlines_and_truncates() {
        let team = empty_team();
        let r = row(1, "p:m", "p:dev", "first\nsecond\nthird");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Inbox),
            "[p:m] first second third"
        );

        let long: String = "x".repeat(300);
        let r = row(1, "s", "r", &long);
        let rendered = render_row(&r, &team, MailboxTab::Inbox);
        // 5 chars ("[s] ") + at most 180 chars of body = 185.
        assert!(rendered.chars().count() <= 185);
    }

    #[test]
    fn render_row_uses_display_name_when_set() {
        // T-160: when the sender id has a `display_name` in the team
        // snapshot, the mailbox row renders the label, not the id.
        // Unknown senders fall through to the raw id (covered above).
        use crate::data::{AgentInfo, TeamSnapshot};
        use team_core::supervisor::AgentState;
        let agent = AgentInfo {
            id: "p:sage".into(),
            agent: "sage".into(),
            project: "p".into(),
            tmux_session: "a-p-sage".into(),
            state: AgentState::Unknown,
            unread_mail: 0,
            pending_approvals: 0,
            is_manager: true,
            display_name: Some("Sage (Visionary)".into()),
            rate_limit_resets_at: None,
            reports_to: None,
        };
        let team = TeamSnapshot {
            root: std::path::PathBuf::from("/tmp"),
            team_name: "t".into(),
            agents: vec![agent],
            channels: vec![],
        };
        let r = row(1, "p:sage", "p:hugo", "ping");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Inbox),
            "[Sage (Visionary)] ping"
        );
    }

    // T-231: tab-aware prefix — Sent shows recipient, others show
    // sender. These pin the contract the operator-visible UX rests on.

    #[test]
    fn render_row_sent_tab_shows_recipient_with_arrow() {
        // Sent rows have the focused agent as sender (constant);
        // recipient is the disambiguating column. Verify the arrow
        // glyph + recipient appear in place of the sender.
        let team = empty_team();
        let r = row(1, "p:me", "p:dev", "ack");
        assert_eq!(render_row(&r, &team, MailboxTab::Sent), "[→p:dev] ack");
    }

    #[test]
    fn render_row_sent_tab_resolves_recipient_display_name() {
        // Same display-name resolution as the Inbox path — the
        // recipient's label, not the raw id, when the team snapshot
        // has a display_name for them.
        use crate::data::{AgentInfo, TeamSnapshot};
        use team_core::supervisor::AgentState;
        let agent = AgentInfo {
            id: "p:hugo".into(),
            agent: "hugo".into(),
            project: "p".into(),
            tmux_session: "a-p-hugo".into(),
            state: AgentState::Running,
            unread_mail: 0,
            pending_approvals: 0,
            is_manager: true,
            display_name: Some("Hugo (PM)".into()),
            rate_limit_resets_at: None,
            reports_to: None,
        };
        let team = TeamSnapshot {
            root: std::path::PathBuf::from("/tmp"),
            team_name: "t".into(),
            agents: vec![agent],
            channels: vec![],
        };
        let r = row(1, "p:sage", "p:hugo", "ping");
        assert_eq!(render_row(&r, &team, MailboxTab::Sent), "[→Hugo (PM)] ping");
    }

    #[test]
    fn render_row_sent_tab_renders_channel_recipient_with_hash() {
        // Broadcast-to-channel rows have `recipient = channel:<id>`.
        // The Sent prefix should render as `→#<short>` — operators
        // recognize `#dev`, not `channel:teamctl:dev`.
        let team = empty_team();
        let r = row(1, "p:me", "channel:teamctl:dev", "rolling 0.8.3");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Sent),
            "[→#dev] rolling 0.8.3"
        );
    }

    #[test]
    fn render_row_sent_tab_renders_user_recipient_verbatim() {
        // Telegram-bound `reply_to_user` rows have `recipient = user:telegram`.
        // No special prefix-stripping — operators already recognize
        // the `user:*` shape and dropping the prefix would lose the
        // "this went to the operator" signal.
        let team = empty_team();
        let r = row(1, "p:mgr", "user:telegram", "PR url");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Sent),
            "[→user:telegram] PR url"
        );
    }

    #[test]
    fn render_row_non_sent_tabs_still_show_sender() {
        // Inbox / Wire prefix is the sender. Channel has its own
        // two-segment shape pinned in the T-249 tests below.
        let team = empty_team();
        let r = row(1, "p:from", "p:me", "yo");
        assert_eq!(render_row(&r, &team, MailboxTab::Inbox), "[p:from] yo");
        assert_eq!(render_row(&r, &team, MailboxTab::Wire), "[p:from] yo");
    }

    // T-249: Channel tab — two bracketed segments, channel then sender.
    // The disambiguator the operator needs is "which channel was this
    // posted in", because the tab folds every subscribed channel into
    // a single feed.

    #[test]
    fn render_row_channel_tab_prefixes_channel_name_and_sender() {
        let team = empty_team();
        let r = row(1, "p:from", "channel:teamctl:dev", "yo");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Channel),
            "[#dev] [p:from] yo"
        );
    }

    #[test]
    fn render_row_channel_tab_resolves_sender_display_name() {
        // Sender resolution mirrors the Inbox path — display_name
        // when set on the team snapshot, raw id otherwise. Channel
        // name resolution is independent.
        use crate::data::{AgentInfo, TeamSnapshot};
        use team_core::supervisor::AgentState;
        let agent = AgentInfo {
            id: "p:wren".into(),
            agent: "wren".into(),
            project: "p".into(),
            tmux_session: "a-p-wren".into(),
            state: AgentState::Running,
            unread_mail: 0,
            pending_approvals: 0,
            is_manager: false,
            display_name: Some("Wren (Engineer)".into()),
            rate_limit_resets_at: None,
            reports_to: None,
        };
        let team = TeamSnapshot {
            root: std::path::PathBuf::from("/tmp"),
            team_name: "t".into(),
            agents: vec![agent],
            channels: vec![],
        };
        let r = row(1, "p:wren", "channel:p:all", "hello");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Channel),
            "[#all] [Wren (Engineer)] hello"
        );
    }

    #[test]
    fn render_row_channel_tab_handles_malformed_channel_recipient() {
        // Defensive — channel_feed SQL only returns rows shaped
        // `channel:<channel_id>`, but if a malformed value ever
        // lands (manual write, future schema shift), the row still
        // renders without panic. Pins recipient_label's malformed
        // fallback (matches T-231's parallel sent-tab test).
        let team = empty_team();
        let r = row(1, "p:from", "channel:malformed", "yo");
        assert_eq!(
            render_row(&r, &team, MailboxTab::Channel),
            "[#malformed] [p:from] yo"
        );
    }

    #[test]
    fn mock_records_calls() {
        let mock = MockMailboxSource {
            inbox_rows: vec![row(1, "p:m", "p:a", "hi")],
            ..Default::default()
        };
        let _ = mock.inbox("p:a", 0).unwrap();
        let _ = mock.sent("p:a", 2).unwrap();
        let _ = mock.channel_feed("p:a", 5).unwrap();
        let _ = mock.wire("p", 9).unwrap();
        assert_eq!(*mock.inbox_calls.lock().unwrap(), vec![("p:a".into(), 0)]);
        assert_eq!(*mock.sent_calls.lock().unwrap(), vec![("p:a".into(), 2)]);
        assert_eq!(*mock.channel_calls.lock().unwrap(), vec![("p:a".into(), 5)]);
        assert_eq!(*mock.wire_calls.lock().unwrap(), vec![("p".into(), 9)]);
    }

    // T-131 PR-1: cursor + visible_indices invariants.

    fn rows_n(n: i64) -> Vec<MessageRow> {
        (1..=n).map(|i| row(i, "p:m", "p:dev", "x")).collect()
    }

    #[test]
    fn visible_indices_is_identity_in_pr1() {
        // PR-1 invariant: visible_indices(tab) == (0..rows(tab).len()).
        // PR-2 swaps the body — this test guards the PR-1 baseline so
        // a PR-2 regression that breaks PR-1's identity assumption
        // surfaces here, not in a render call site downstream.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(5));
        assert_eq!(buf.visible_indices(MailboxTab::Inbox), vec![0, 1, 2, 3, 4]);
        assert!(buf.visible_indices(MailboxTab::Sent).is_empty());
    }

    #[test]
    fn extend_into_empty_seats_cursor_at_tail() {
        // Pre-T-131 UX was "tail to whatever fits"; the cursor seat
        // preserves it — a freshly-populated tab shows the latest row
        // selected, matching the existing snapshot expectations for
        // unfocused mailbox panes.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(7));
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 6);
    }

    #[test]
    fn extend_when_cursor_at_tail_follows_new_arrivals() {
        // Standard chat-app "follow tail" UX: as long as the operator
        // hasn't scrolled away, new messages keep the cursor at the
        // newest row.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(3));
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 2);
        buf.extend(
            MailboxTab::Inbox,
            vec![row(4, "p:m", "p:dev", "x"), row(5, "p:m", "p:dev", "x")],
        );
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 4);
    }

    #[test]
    fn extend_when_cursor_scrolled_up_does_not_follow() {
        // Operator inspecting older history shouldn't be yanked back
        // to the tail by a new arrival — the cursor is sticky once it
        // leaves the tail.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(5));
        buf.cursor_home(MailboxTab::Inbox); // selected_idx = 0
        buf.extend(MailboxTab::Inbox, vec![row(6, "p:m", "p:dev", "x")]);
        assert_eq!(
            buf.cursor(MailboxTab::Inbox).selected_idx,
            0,
            "scrolled-up cursor must not jump on new arrival"
        );
    }

    #[test]
    fn extend_reclamps_cursor_after_drain() {
        // The MAX_TAB_ROWS drain shifts indices — a cursor that was
        // valid pre-drain must be re-clamped against the new visible
        // length so render never indexes past the buffer.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(MAX_TAB_ROWS as i64));
        buf.cursor_home(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
        // Push another batch large enough to drain off the front.
        let next: Vec<MessageRow> = (501..=510).map(|i| row(i, "p:m", "p:dev", "x")).collect();
        buf.extend(MailboxTab::Inbox, next);
        let visible = buf.visible_indices(MailboxTab::Inbox);
        assert_eq!(visible.len(), MAX_TAB_ROWS);
        assert!(
            buf.cursor(MailboxTab::Inbox).selected_idx < visible.len(),
            "post-drain cursor must stay in range; got {}, visible.len {}",
            buf.cursor(MailboxTab::Inbox).selected_idx,
            visible.len()
        );
    }

    #[test]
    fn move_cursor_down_and_up_clamp_at_ends() {
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(3)); // cursor seated at 2
        buf.move_cursor_down(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 2, "tail clamps");
        buf.move_cursor_up(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 1);
        buf.move_cursor_up(MailboxTab::Inbox);
        buf.move_cursor_up(MailboxTab::Inbox);
        buf.move_cursor_up(MailboxTab::Inbox); // extra up at 0 is no-op
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0, "head clamps");
    }

    #[test]
    fn page_cursor_jumps_a_screen() {
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(50));
        buf.cursor_home(MailboxTab::Inbox);
        buf.page_cursor_down(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, PAGE_JUMP);
        buf.page_cursor_down(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 2 * PAGE_JUMP);
        buf.page_cursor_up(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, PAGE_JUMP);
        // PageDown past the tail clamps.
        for _ in 0..20 {
            buf.page_cursor_down(MailboxTab::Inbox);
        }
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 49);
        // PageUp past the head clamps.
        for _ in 0..20 {
            buf.page_cursor_up(MailboxTab::Inbox);
        }
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
    }

    #[test]
    fn cursor_home_and_end_jump_to_ends() {
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(20));
        buf.cursor_home(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
        buf.cursor_end(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 19);
    }

    #[test]
    fn cursors_are_per_tab_and_independent() {
        // Issue AC: "Scrolling is per-tab — Inbox/Sent/Channel/Wire
        // each remember their own position."
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(10));
        buf.extend(MailboxTab::Sent, rows_n(10));
        buf.cursor_home(MailboxTab::Inbox); // Inbox cursor at 0
                                            // Sent cursor stays at its post-extend tail (idx 9).
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
        assert_eq!(buf.cursor(MailboxTab::Sent).selected_idx, 9);
        // And channel/wire are still at 0 with empty buffers.
        assert_eq!(buf.cursor(MailboxTab::Channel).selected_idx, 0);
        assert_eq!(buf.cursor(MailboxTab::Wire).selected_idx, 0);
    }

    #[test]
    fn reset_clears_cursors_too() {
        // Reset is called when the focused agent changes; the new
        // agent's mailbox starts from a clean slate, cursor at 0.
        let mut buf = MailboxBuffers::default();
        buf.extend(MailboxTab::Inbox, rows_n(5));
        buf.cursor_home(MailboxTab::Inbox);
        buf.move_cursor_down(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 1);
        buf.reset();
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
        assert_eq!(buf.cursor(MailboxTab::Sent).selected_idx, 0);
    }

    #[test]
    fn cursor_methods_are_safe_on_empty_buffer() {
        // No rows yet — every cursor method must be a no-op on
        // selected_idx = 0 rather than panic.
        let mut buf = MailboxBuffers::default();
        buf.move_cursor_down(MailboxTab::Inbox);
        buf.move_cursor_up(MailboxTab::Inbox);
        buf.page_cursor_down(MailboxTab::Inbox);
        buf.page_cursor_up(MailboxTab::Inbox);
        buf.cursor_home(MailboxTab::Inbox);
        buf.cursor_end(MailboxTab::Inbox);
        assert_eq!(buf.cursor(MailboxTab::Inbox).selected_idx, 0);
    }
}
