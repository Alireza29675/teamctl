//! Library half of the `teamctl-ui` crate. Holds the app loop and the
//! widget rendering primitives so integration tests can drive them
//! against a `ratatui::buffer::Buffer` without spinning up a real
//! terminal. The thin `main.rs` binary owns terminal lifecycle only.

pub mod app;
pub mod approvals;
pub mod compose;
pub mod data;
pub mod help;
pub mod init_picker;
pub mod keysender;
pub mod layouts;
pub mod mailbox;
pub mod onboarding;
pub mod pane;
pub mod pane_resize;
pub mod splash;
pub mod status_bar;
pub mod statusline;
pub mod theme;
pub mod triptych;
pub mod tutorial;
pub mod watch;
