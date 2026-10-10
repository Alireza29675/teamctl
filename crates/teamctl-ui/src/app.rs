//! Public application API, with state, input, runtime, and rendering modules.

use std::time::Duration;

const SPLASH_AUTO_DISMISS: Duration = Duration::from_secs(3);
/// Idle ceiling for the event loop's `event::poll` — also the loop's
/// redraw cadence when nothing is typed (each iteration redraws, then
/// blocks here until a key arrives or this elapses). `event::poll` is
/// fd-event-driven, so a keypress wakes it immediately regardless of
/// this value (it is NOT the input-latency source, #386); lowering it
/// only makes the idle redraw + file-watcher dirty-detect snappier. At
/// ~30 Hz the per-iteration work is a sub-millisecond ratatui diff
/// (no subprocess unless a capture/refresh is due), so the idle CPU
/// cost stays negligible.
const POLL_INTERVAL: Duration = Duration::from_millis(33);
/// How often the team snapshot + detail-pane capture get refreshed.
/// PR-UI-2 polls; PR-UI-3 may upgrade to event subscriptions.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// How often the *focused* agent's pane is re-captured on its own. The
/// full 1s refresh is too slow for the detail view to feel live, so we
/// re-capture just the one focused pane this often between refreshes —
const PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_DETAIL_LINES: usize = 2000;

mod actions;
mod events;
mod navigation;
mod render;
mod runtime;
mod state;

pub use events::handle_event;
pub use render::{draw, render_to_buffer};
#[cfg(test)]
use runtime::recapture_focused_pane;
pub use runtime::{refresh, refresh_approvals, refresh_mailbox, run, sync_focused_pane_size_to};
pub use state::{App, SplitOrientation, Stage};

#[cfg(test)]
mod tests;
