//! Durable operator conversations, independent of the dashboard mailbox.
//!
//! State and rendering are synchronous and deterministic. Only the bounded
//! worker touches SQLite; its results retain the conversation/request identity.

mod input;
mod render;
mod responses;
mod state;
mod worker;

pub use render::{geometry, ChatView};
pub use state::{ChatState, Focus, Selection, ThreadState};
pub use worker::{ChatWorker, Request, Response};

pub const HISTORY_PAGE: usize = 100;
pub const HISTORY_CAP: usize = 1000;
pub const QUEUE_CAP: usize = 16;

#[cfg(test)]
mod tests;
