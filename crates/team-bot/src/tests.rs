use super::*;
use rusqlite::Connection;

// Feature files share these fixture helpers in the test module.
include!("tests/inbound.rs");
include!("tests/approval_ui.rs");
include!("tests/repository.rs");
include!("tests/transport.rs");
include!("tests/render.rs");
include!("tests/delivery.rs");
include!("tests/startup.rs");
include!("tests/tmux.rs");
include!("tests/controls.rs");
include!("tests/media.rs");
include!("tests/voice.rs");
include!("tests/rich_cli.rs");
