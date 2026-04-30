//! Library half of the `teamctl-ui` crate. Holds the app loop and the
//! widget rendering primitives so integration tests can drive them
//! against a `ratatui::buffer::Buffer` without spinning up a real
//! terminal. The thin `main.rs` binary owns terminal lifecycle only.

pub mod app;
pub mod approvals;
pub mod compose;
pub mod data;
pub mod mailbox;
pub mod pane;
pub mod splash;
pub mod statusline;
pub mod theme;
pub mod triptych;
pub mod tutorial;
pub mod watch;
