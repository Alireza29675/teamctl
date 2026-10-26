//! Bounded database work without blocking the render/input loop.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use team_core::chat::{ChatMessage, Conversation, Repository};

use super::{HISTORY_PAGE, QUEUE_CAP};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    List,
    Open {
        agent_id: String,
        selection_epoch: u64,
    },
    History {
        conversation_id: String,
        before: Option<i64>,
    },
    RefreshHistory {
        conversation_id: String,
        after: i64,
    },
    NewMessages {
        conversation_id: String,
        after: i64,
    },
    RecentStatuses {
        conversation_id: String,
        before: i64,
    },
    Send {
        conversation_id: String,
        text: String,
        request_id: String,
    },
}

#[derive(Debug)]
pub enum Response {
    Listed(Result<Vec<Conversation>, String>),
    Opened {
        selection_epoch: u64,
        result: Result<Conversation, String>,
    },
    History {
        conversation_id: String,
        before: Option<i64>,
        result: Result<Vec<ChatMessage>, String>,
    },
    RefreshedHistory {
        conversation_id: String,
        result: Result<Vec<ChatMessage>, String>,
    },
    NewMessages {
        conversation_id: String,
        after: i64,
        result: Result<Vec<ChatMessage>, String>,
    },
    Sent {
        conversation_id: String,
        request_id: String,
        result: Result<ChatMessage, String>,
    },
}

pub struct ChatWorker {
    requests: SyncSender<Request>,
    responses: Receiver<Response>,
}

impl ChatWorker {
    pub fn new(path: PathBuf) -> Self {
        let (requests, incoming) = mpsc::sync_channel(QUEUE_CAP);
        let (outgoing, responses) = mpsc::sync_channel(QUEUE_CAP);
        std::thread::spawn(move || {
            // Open per request so a transient startup/locking failure can recover
            // and an atomic replacement of the broker never leaves a stale handle.
            while let Ok(request) = incoming.recv() {
                if outgoing.send(execute(&path, request)).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            responses,
        }
    }

    pub fn try_submit(&self, request: Request) -> Result<(), Request> {
        self.requests
            .try_send(request)
            .map_err(|error| match error {
                TrySendError::Full(request) | TrySendError::Disconnected(request) => request,
            })
    }

    pub fn try_receive(&self) -> Option<Response> {
        self.responses.try_recv().ok()
    }
}

fn execute(path: &std::path::Path, request: Request) -> Response {
    match request {
        Request::List => {
            Response::Listed(query(path, |repository| repository.list_conversations(200)))
        }
        Request::Open {
            agent_id,
            selection_epoch,
        } => Response::Opened {
            selection_epoch,
            result: query(path, |repository| repository.open_local(&agent_id)),
        },
        Request::History {
            conversation_id,
            before,
        } => Response::History {
            result: query(path, |repository| {
                repository.history(&conversation_id, before, None, HISTORY_PAGE)
            }),
            conversation_id,
            before,
        },
        Request::RefreshHistory {
            conversation_id,
            after,
        } => Response::RefreshedHistory {
            result: query(path, |repository| {
                repository.history(&conversation_id, None, Some(after), HISTORY_PAGE)
            }),
            conversation_id,
        },
        Request::RecentStatuses {
            conversation_id,
            before,
        } => Response::RefreshedHistory {
            result: query(path, |repository| {
                repository.history(&conversation_id, Some(before), None, HISTORY_PAGE)
            }),
            conversation_id,
        },
        Request::NewMessages {
            conversation_id,
            after,
        } => Response::NewMessages {
            result: query(path, |repository| {
                repository.history(&conversation_id, None, Some(after), HISTORY_PAGE)
            }),
            conversation_id,
            after,
        },
        Request::Send {
            conversation_id,
            text,
            request_id,
        } => Response::Sent {
            result: query(path, |repository| {
                repository.send_local(&conversation_id, &text, &request_id)
            }),
            conversation_id,
            request_id,
        },
    }
}

fn query<T>(
    path: &std::path::Path,
    operation: impl FnOnce(&Repository<'_>) -> team_core::chat::Result<T>,
) -> Result<T, String> {
    let connection = team_core::chat::open(path).map_err(|error| error.to_string())?;
    operation(&Repository::new(&connection)).map_err(|error| error.to_string())
}
