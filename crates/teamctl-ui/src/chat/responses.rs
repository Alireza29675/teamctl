//! Apply asynchronous results to their original conversation and request.

use super::state::merge_messages;
use super::{ChatState, Focus, Request, Response, Selection, HISTORY_PAGE};
use crate::compose::Editor;

impl ChatState {
    pub fn apply(&mut self, response: Response) {
        match response {
            Response::Listed(result) => {
                self.list_pending = false;
                match result {
                    Ok(conversations) => {
                        self.list_error = None;
                        // Preserve an open thread even if it falls outside the bounded
                        // recent list; a refresh must not remove its target or draft.
                        for conversation in conversations {
                            self.upsert_conversation(conversation);
                        }
                        if self.selection.is_none() && !self.open_pending {
                            if let Some(id) = self
                                .conversations
                                .first()
                                .map(|conversation| conversation.id.clone())
                            {
                                self.select(Selection::Conversation(id));
                            }
                        }
                    }
                    Err(error) => self.list_error = Some(error),
                }
            }
            Response::Opened {
                selection_epoch,
                result,
            } => {
                self.open_requests.remove(&selection_epoch);
                self.open_pending = !self.open_requests.is_empty();
                match result {
                    Ok(conversation) => {
                        let id = conversation.id.clone();
                        self.upsert_conversation(conversation);
                        if selection_epoch == self.selection_epoch {
                            self.select(Selection::Conversation(id));
                            self.focus = Focus::Composer;
                            self.error = None;
                        }
                    }
                    Err(error) if selection_epoch == self.selection_epoch => {
                        self.error = Some(error)
                    }
                    Err(_) => {}
                }
            }
            Response::History {
                conversation_id,
                before,
                result,
            } => {
                let thread = self.threads.entry(conversation_id).or_default();
                // The FIFO worker may still have older pages queued when the
                // user requests the tail. They cannot repopulate that window.
                if before.is_some() && thread.reload_tail {
                    return;
                }
                if before.is_some() {
                    thread.paging = false;
                } else {
                    thread.refreshing = false;
                }
                match result {
                    Ok(messages) => {
                        if before.is_some() || thread.messages.is_empty() {
                            thread.has_older = messages.len() == HISTORY_PAGE;
                        }
                        if before.is_none() && thread.reload_tail && thread.anchor.is_none() {
                            thread.messages.clear();
                            thread.newest_seen = None;
                            thread.reload_tail = false;
                            thread.unread = 0;
                        }
                        merge_messages(thread, messages, before.is_some());
                        thread.error = None;
                    }
                    Err(error) => thread.error = Some(error),
                }
            }
            Response::RefreshedHistory {
                conversation_id,
                result,
            } => {
                let thread = self.threads.entry(conversation_id).or_default();
                if thread.reload_tail {
                    return;
                }
                match result {
                    Ok(messages) => {
                        let cursor = thread.newest_seen;
                        merge_messages(thread, messages, true);
                        thread.newest_seen = cursor;
                    }
                    Err(error) => thread.error = Some(error),
                }
            }
            Response::NewMessages {
                conversation_id,
                after,
                result,
            } => {
                let thread = self.threads.entry(conversation_id.clone()).or_default();
                if thread.newest_seen != Some(after) {
                    return;
                }
                thread.new_pending = false;
                match result {
                    Ok(messages) => {
                        let more = messages.len() == HISTORY_PAGE;
                        if thread.tail_evicted() {
                            // Preserve a contiguous historical window even if a
                            // short new reply fits under its byte/count budget.
                            // Its durable body will appear in the tail reload.
                            if thread.anchor.is_some() {
                                thread.unread = thread.unread.saturating_add(messages.len());
                            }
                            if let Some(last) = messages.last() {
                                thread.newest_seen = Some(last.id);
                            }
                        } else {
                            merge_messages(thread, messages, false);
                        }
                        thread.error = None;
                        if more {
                            if let Some(after) = thread.newest_seen {
                                if self.enqueue(Request::NewMessages {
                                    conversation_id: conversation_id.clone(),
                                    after,
                                }) {
                                    self.threads.entry(conversation_id).or_default().new_pending =
                                        true;
                                }
                            }
                        }
                    }
                    Err(error) => thread.error = Some(error),
                }
            }
            Response::Sent {
                conversation_id,
                request_id,
                result,
            } => {
                let thread = self.threads.entry(conversation_id).or_default();
                let Some(submission) = thread
                    .submission
                    .as_mut()
                    .filter(|submission| submission.request_id == request_id)
                else {
                    return;
                };
                submission.pending = false;
                match result {
                    Ok(message) => {
                        if thread.editor.body() == submission.text {
                            thread.editor = Editor::default();
                        }
                        thread.submission = None;
                        if !thread.tail_evicted() && !thread.reload_tail {
                            let cursor = thread.newest_seen;
                            merge_messages(thread, vec![message], true);
                            thread.newest_seen = cursor;
                        }
                    }
                    Err(error) => submission.error = Some(error),
                }
            }
        }
    }
}
