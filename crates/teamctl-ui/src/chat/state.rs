//! Conversation identity, drafts, paging, and submission state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use team_core::chat::{ChatMessage, Conversation};

use super::{Request, HISTORY_CAP, QUEUE_CAP};
use crate::compose::Editor;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    Conversations,
    History,
    #[default]
    Composer,
}

impl Focus {
    pub fn next(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Conversations, false) | (Self::Composer, true) => Self::History,
            (Self::History, false) | (Self::Conversations, true) => Self::Composer,
            (Self::Composer, false) | (Self::History, true) => Self::Conversations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Conversation(String),
    NewAgent(String),
}

#[derive(Debug, Clone)]
pub struct Submission {
    pub request_id: String,
    pub text: String,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ThreadState {
    pub editor: Editor,
    pub messages: Vec<ChatMessage>,
    /// Message identity plus wrapped-line offset anchors a historical viewport.
    pub anchor: Option<(i64, usize)>,
    pub unread: usize,
    pub submission: Option<Submission>,
    pub error: Option<String>,
    pub refreshing: bool,
    pub new_pending: bool,
    pub paging: bool,
    pub has_older: bool,
    pub newest_seen: Option<i64>,
    pub reload_tail: bool,
    pub(super) render_cache: std::cell::RefCell<super::render::TranscriptCache>,
}

impl ThreadState {
    /// The delivery cursor can advance beyond a retained historical window.
    pub(super) fn tail_evicted(&self) -> bool {
        self.messages
            .last()
            .zip(self.newest_seen)
            .is_some_and(|(last, newest)| last.id < newest)
    }
}

pub struct ChatState {
    pub conversations: Vec<Conversation>,
    pub threads: BTreeMap<String, ThreadState>,
    pub selection: Option<Selection>,
    pub focus: Focus,
    pub error: Option<String>,
    pub selection_epoch: u64,
    pub list_error: Option<String>,
    pub last_refresh: Instant,
    pub list_pending: bool,
    pub open_pending: bool,
    pub(super) open_requests: BTreeSet<u64>,
    pub(crate) outbox: VecDeque<Request>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            conversations: Vec::new(),
            threads: BTreeMap::new(),
            selection: None,
            focus: Focus::Composer,
            error: None,
            selection_epoch: 0,
            list_error: None,
            last_refresh: Instant::now(),
            list_pending: false,
            open_pending: false,
            open_requests: BTreeSet::new(),
            outbox: VecDeque::new(),
        }
    }
}

impl ChatState {
    pub fn selected_id(&self) -> Option<&str> {
        match self.selection.as_ref() {
            Some(Selection::Conversation(id)) => Some(id),
            _ => None,
        }
    }

    pub fn conversation(&self) -> Option<&Conversation> {
        let id = self.selected_id()?;
        self.conversations
            .iter()
            .find(|conversation| conversation.id == id)
    }

    pub fn thread(&self) -> Option<&ThreadState> {
        self.threads.get(self.selected_id()?)
    }

    pub fn thread_mut(&mut self) -> Option<&mut ThreadState> {
        let id = self.selected_id()?.to_string();
        Some(self.threads.entry(id).or_default())
    }

    pub fn select(&mut self, selection: Selection) {
        if self.selection.as_ref() != Some(&selection) {
            self.selection_epoch = self.selection_epoch.wrapping_add(1);
            self.selection = Some(selection);
        }
        self.refresh_selected();
    }

    pub fn entries(&self, agents: &[String]) -> Vec<Selection> {
        self.conversations
            .iter()
            .map(|conversation| Selection::Conversation(conversation.id.clone()))
            .chain(agents.iter().map(|id| Selection::NewAgent(id.clone())))
            .collect()
    }

    pub fn navigate(&mut self, agents: &[String], backwards: bool) {
        let entries = self.entries(agents);
        if entries.is_empty() {
            return;
        }
        let index = entries
            .iter()
            .position(|entry| Some(entry) == self.selection.as_ref())
            .unwrap_or(0);
        let next = if backwards {
            index.checked_sub(1).unwrap_or(entries.len() - 1)
        } else {
            (index + 1) % entries.len()
        };
        self.select(entries[next].clone());
    }

    pub fn open_agent(&mut self, agent_id: String) {
        if self.open_requests.contains(&self.selection_epoch) {
            return;
        }
        if self.enqueue(Request::Open {
            agent_id,
            selection_epoch: self.selection_epoch,
        }) {
            self.open_pending = true;
            self.open_requests.insert(self.selection_epoch);
        }
    }

    pub fn refresh(&mut self) {
        if !self.list_pending && self.enqueue(Request::List) {
            self.list_pending = true;
        }
        self.refresh_selected();
        self.last_refresh = Instant::now();
    }

    pub fn refresh_selected(&mut self) {
        let Some(id) = self.selected_id().map(str::to_string) else {
            return;
        };
        let thread = self.threads.entry(id.clone()).or_default();
        if thread.refreshing || thread.new_pending {
            return;
        }
        if let Some(newest) = thread.newest_seen {
            let anchor = thread.anchor;
            let last = thread.messages.last().map(|message| message.id);
            if self.enqueue(Request::NewMessages {
                conversation_id: id.clone(),
                after: newest,
            }) {
                self.threads.entry(id.clone()).or_default().new_pending = true;
            }
            if let Some((anchor, _)) = anchor {
                self.enqueue(Request::RefreshHistory {
                    conversation_id: id,
                    after: anchor.saturating_sub(1),
                });
            } else if let Some(last) = last {
                self.enqueue(Request::RecentStatuses {
                    conversation_id: id,
                    before: last.saturating_add(1),
                });
            }
            return;
        }
        if self.enqueue(Request::History {
            conversation_id: id.clone(),
            before: None,
        }) {
            self.threads.entry(id.clone()).or_default().refreshing = true;
            if let Some((anchor, _)) = self.threads.get(&id).and_then(|thread| thread.anchor) {
                self.enqueue(Request::RefreshHistory {
                    conversation_id: id,
                    after: anchor.saturating_sub(1),
                });
            }
        }
    }

    pub fn page_older(&mut self) {
        let Some(id) = self.selected_id().map(str::to_string) else {
            return;
        };
        let thread = self.threads.entry(id.clone()).or_default();
        if thread.paging || !thread.has_older {
            return;
        }
        let Some(before) = thread.messages.first().map(|message| message.id) else {
            return;
        };
        if self.enqueue(Request::History {
            conversation_id: id.clone(),
            before: Some(before),
        }) {
            self.threads.entry(id).or_default().paging = true;
        }
    }

    pub fn send(&mut self, known_agents: &[String]) {
        let Some(conversation) = self.conversation() else {
            return;
        };
        if conversation.adapter_id != "local" || conversation.peer_id != "operator" {
            self.error =
                Some("Remote conversation is read-only; open a local chat with the agent.".into());
            return;
        }
        if !known_agents.contains(&conversation.agent_id) {
            self.error = Some("Agent was removed; history is still available.".into());
            return;
        }
        let id = conversation.id.clone();
        let thread = self.threads.entry(id.clone()).or_default();
        let text = thread.editor.body();
        if text.trim().is_empty()
            || thread
                .submission
                .as_ref()
                .is_some_and(|submission| submission.pending)
        {
            return;
        }
        let request_id = thread
            .submission
            .as_ref()
            .filter(|submission| submission.text == text)
            .map(|submission| submission.request_id.clone())
            .unwrap_or_else(request_id);
        if self.enqueue(Request::Send {
            conversation_id: id.clone(),
            text: text.clone(),
            request_id: request_id.clone(),
        }) {
            self.error = None;
            self.threads.entry(id).or_default().submission = Some(Submission {
                request_id,
                text,
                pending: true,
                error: None,
            });
        }
    }

    pub(crate) fn enqueue(&mut self, request: Request) -> bool {
        if self.outbox.len() >= QUEUE_CAP {
            self.error = Some("Chat is busy; retry shortly.".into());
            false
        } else {
            self.outbox.push_back(request);
            true
        }
    }

    pub fn upsert_conversation(&mut self, conversation: Conversation) {
        if let Some(existing) = self
            .conversations
            .iter_mut()
            .find(|existing| existing.id == conversation.id)
        {
            *existing = conversation;
        } else {
            self.conversations.push(conversation);
        }
    }
}

pub(super) fn merge_messages(thread: &mut ThreadState, messages: Vec<ChatMessage>, older: bool) {
    let mut changed = false;
    let newest = thread
        .newest_seen
        .or_else(|| thread.messages.last().map(|message| message.id));
    let mut merged: BTreeMap<i64, ChatMessage> = thread
        .messages
        .drain(..)
        .map(|message| (message.id, message))
        .collect();
    for message in messages {
        if !older
            && thread.anchor.is_some()
            && newest.is_some_and(|id| message.id > id)
            && !merged.contains_key(&message.id)
        {
            thread.unread = thread.unread.saturating_add(1);
        }
        thread.newest_seen = Some(
            thread
                .newest_seen
                .map_or(message.id, |id| id.max(message.id)),
        );
        if merged
            .get(&message.id)
            .is_none_or(|existing| message.revision >= existing.revision)
        {
            changed |= merged.get(&message.id) != Some(&message);
            merged.insert(message.id, message);
        }
    }
    // Keep the viewed historical range while paging; refreshes at the tail
    // retain the latest range. The durable database keeps all older rows.
    while merged.len() > HISTORY_CAP {
        evict_message(thread, &mut merged);
    }
    let mut bytes: usize = merged.values().map(|message| message.text.len()).sum();
    while bytes > 2 * 1024 * 1024 && merged.len() > 1 {
        let dropped = evict_message(thread, &mut merged);
        if let Some((_, message)) = dropped {
            bytes = bytes.saturating_sub(message.text.len());
        }
    }
    thread.messages = merged.into_values().collect();
    if changed {
        *thread.render_cache.get_mut() = super::render::TranscriptCache::default();
    }
}

fn evict_message(
    thread: &mut ThreadState,
    messages: &mut BTreeMap<i64, ChatMessage>,
) -> Option<(i64, ChatMessage)> {
    if thread.anchor.is_some_and(|(anchor, _)| {
        messages
            .last_key_value()
            .is_some_and(|(id, _)| *id > anchor)
    }) {
        messages.pop_last()
    } else {
        thread.has_older = true;
        messages.pop_first()
    }
}

fn request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "ui:{}:{time}:{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
