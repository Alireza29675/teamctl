//! Mailbox-pane data source and tab definitions.
//!
//! Three filter shapes, one per tab in SPEC §2's Triptych mailbox:
//!
//! - `Inbox` — DMs whose `recipient = '<project>:<agent>'`.
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
//! learn it.

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxTab {
    Inbox,
    Channel,
    Wire,
}

impl MailboxTab {
    pub const ALL: [MailboxTab; 3] = [MailboxTab::Inbox, MailboxTab::Channel, MailboxTab::Wire];

    pub fn label(self) -> &'static str {
        match self {
            MailboxTab::Inbox => "Inbox",
            MailboxTab::Channel => "Channel",
            MailboxTab::Wire => "Wire",
        }
    }

    pub fn empty_hint(self) -> &'static str {
        match self {
            MailboxTab::Inbox => "(no DMs)",
            MailboxTab::Channel => "(no channel traffic)",
            MailboxTab::Wire => "(quiet)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            MailboxTab::Inbox => MailboxTab::Channel,
            MailboxTab::Channel => MailboxTab::Wire,
            MailboxTab::Wire => MailboxTab::Inbox,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            MailboxTab::Inbox => MailboxTab::Wire,
            MailboxTab::Channel => MailboxTab::Inbox,
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

/// Format a single row for the mailbox pane. Kept terse: `[from]
/// text` on one line — no timestamps, no recipient (the tab tells
/// you the recipient class). Multi-line bodies are flattened with a
/// space so a single message stays one row in the pane.
pub fn render_row(row: &MessageRow) -> String {
    let one_line: String = row
        .text
        .replace('\n', " ")
        .replace('\r', "")
        .chars()
        .take(180)
        .collect();
    format!("[{}] {}", row.sender, one_line)
}

/// Lookup contract: each method returns rows newer than `after_id`
/// for the given filter, in ascending id order. Callers fold the
/// returned rows into a per-tab buffer and bump `after_id` to the
/// last returned id.
pub trait MailboxSource: Send + Sync {
    fn inbox(&self, agent_id: &str, after_id: i64) -> Result<Vec<MessageRow>>;
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

/// Per-agent buffer state — three tabs, three `after_id` cursors.
/// Lives on `App` so swapping the focused agent resets the cursors
/// without trying to back-fill: the operator sees only forward
/// motion in the tab they're watching.
#[derive(Debug, Default, Clone)]
pub struct MailboxBuffers {
    pub inbox: Vec<MessageRow>,
    pub channel: Vec<MessageRow>,
    pub wire: Vec<MessageRow>,
    pub inbox_after: i64,
    pub channel_after: i64,
    pub wire_after: i64,
}

const MAX_TAB_ROWS: usize = 500;

impl MailboxBuffers {
    pub fn rows(&self, tab: MailboxTab) -> &[MessageRow] {
        match tab {
            MailboxTab::Inbox => &self.inbox,
            MailboxTab::Channel => &self.channel,
            MailboxTab::Wire => &self.wire,
        }
    }

    /// Fold a freshly-fetched batch into the appropriate tab,
    /// trimming to the last `MAX_TAB_ROWS`. Bumps the cursor to the
    /// last returned id when the batch is non-empty.
    pub fn extend(&mut self, tab: MailboxTab, batch: Vec<MessageRow>) {
        let last_id = batch.last().map(|r| r.id);
        let (buf, after) = match tab {
            MailboxTab::Inbox => (&mut self.inbox, &mut self.inbox_after),
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
    }

    /// Reset every tab's contents and cursor. Called when the
    /// focused agent changes — the new agent's `inbox` filter would
    /// otherwise skip historical rows that landed before our last
    /// `inbox_after`.
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
        pub channel_rows: Vec<MessageRow>,
        pub wire_rows: Vec<MessageRow>,
        pub inbox_calls: Mutex<Vec<(String, i64)>>,
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
    fn next_cycles_inbox_channel_wire_inbox() {
        let mut t = MailboxTab::Inbox;
        t = t.next();
        assert_eq!(t, MailboxTab::Channel);
        t = t.next();
        assert_eq!(t, MailboxTab::Wire);
        t = t.next();
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

    #[test]
    fn render_row_flattens_newlines_and_truncates() {
        let r = row(1, "p:m", "p:dev", "first\nsecond\nthird");
        assert_eq!(render_row(&r), "[p:m] first second third");

        let long: String = "x".repeat(300);
        let r = row(1, "s", "r", &long);
        let rendered = render_row(&r);
        // 5 chars ("[s] ") + at most 180 chars of body = 185.
        assert!(rendered.chars().count() <= 185);
    }

    #[test]
    fn mock_records_calls() {
        let mock = MockMailboxSource {
            inbox_rows: vec![row(1, "p:m", "p:a", "hi")],
            ..Default::default()
        };
        let _ = mock.inbox("p:a", 0).unwrap();
        let _ = mock.channel_feed("p:a", 5).unwrap();
        let _ = mock.wire("p", 9).unwrap();
        assert_eq!(*mock.inbox_calls.lock().unwrap(), vec![("p:a".into(), 0)]);
        assert_eq!(*mock.channel_calls.lock().unwrap(), vec![("p:a".into(), 5)]);
        assert_eq!(*mock.wire_calls.lock().unwrap(), vec![("p".into(), 9)]);
    }
}
