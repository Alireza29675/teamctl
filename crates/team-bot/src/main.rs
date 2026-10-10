//! `team-bot` — Telegram adapter for the teamctl `interfaces:` abstraction.
//!
//! Watches the mailbox for messages addressed to managers with an
//! `interfaces.telegram` block (and for new pending approvals), and
//! surfaces both to the authorized Telegram chat. Inbound user
//! messages (DMs + callback button taps) write back into the mailbox.
//!
//! Later interface adapters (`team-interface-discord`, `-imessage`, `-cli`)
//! mirror this crate's shape: an async loop against the same SQLite mailbox
//! plus an adapter-specific transport.

use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
#[cfg(test)]
use clap::Parser;
#[cfg(test)]
use rusqlite::params;
use rusqlite::Connection;
#[cfg(test)]
use teloxide::prelude::*;
use teloxide::types::ChatId;
#[cfg(test)]
use teloxide::types::{InlineKeyboardMarkup, MessageId};
use tokio::sync::Mutex;

mod config;
#[cfg(test)]
use config::*;
mod download;
#[cfg(test)]
use download::*;
mod repository;
mod startup;
#[cfg(test)]
use repository::*;
mod inbound;
#[cfg(test)]
use inbound::*;
mod approval;
#[cfg(test)]
use approval::*;
mod outbound;
#[cfg(test)]
use outbound::*;
mod transport;
#[cfg(test)]
use transport::*;
mod tmux;
#[cfg(test)]
use tmux::*;
mod media;
#[cfg(test)]
use media::*;
mod render;
#[cfg(test)]
use render::*;
mod voice;
#[cfg(test)]
use voice::*;

struct State {
    conn: Mutex<Connection>,
    allow: Vec<i64>,
    /// `<project>:<manager>` if this instance is scoped; otherwise all managers.
    manager: Option<String>,
    /// T-367: friendly label for the scoped manager (from `display_name`,
    /// T-160). Preferred over `manager` in the first-connect greeting; falls
    /// back to the `<project>:<manager>` id when unset.
    manager_display_name: Option<String>,
    /// Tmux session prefix used by slash-passthrough to compute the manager's
    /// session name. Stored on `State` so handle_message can reach it without
    /// re-reading the CLI args.
    tmux_prefix: String,
    /// Directory to write inbound media downloads under (T-086-C). Resolved
    /// from the mailbox path's parent (`<root>/.team/state/inbound-media/`)
    /// at startup so the bot stays self-contained — no extra CLI flag, no
    /// config sync.
    media_root: PathBuf,
    /// T-101 voice STT: when present, inbound voice notes get transcribed
    /// and forwarded to the manager prefixed with `VOICE_INBOX_PREFIX`.
    /// `None` means voice messages stay unhandled (the bot was started
    /// without `speech_to_text` configured, or the API key was unset at
    /// spawn time).
    stt: Option<SttRuntime>,
    /// T-102 typing windows: per-chat deadline for the active "typing…"
    /// indicator. Keyed by `ChatId`; value is the `Instant` after which
    /// the window has expired. Empty when no agent has called
    /// `show_typing` recently. The outbound dispatcher writes here when
    /// it sees a `kind = "typing"` row, the typing-refresh task reads +
    /// drops expired entries every ~4s, and any text/image/file
    /// dispatch clears the entry for that chat so the indicator
    /// disappears the moment a real message lands.
    typing: Mutex<HashMap<ChatId, Instant>>,
    /// #479 / rich-default-on: master switch for rich sends — on unless
    /// `--no-rich-messages` / `TEAMCTL_TELEGRAM_NO_RICH`. When on, a Text row
    /// *without* a reply target is sent via `sendRichMessage` (rich markdown)
    /// with a transparent fallback to the HTML sender on failure; replies always
    /// use the HTML sender to keep Telegram threading. See `use_rich_path`.
    rich: bool,
}

/// T-102: ceiling on a single typing window. Telegram's
/// `sendChatAction("typing")` only persists ~5s on its own; we re-fire
/// every `TYPING_REFRESH_INTERVAL` while the window is active, capped at
/// `TYPING_WINDOW_CEILING` so a forgotten clear can't pin the indicator
/// open indefinitely.
const TYPING_WINDOW_CEILING: Duration = Duration::from_secs(10);
const TYPING_REFRESH_INTERVAL: Duration = Duration::from_secs(4);

/// T-101: resolved STT settings the voice handler needs at request time.
/// Constructed once at startup from the four `--stt-*` flags.
struct SttRuntime {
    provider: String,
    api_key: String,
    model: String,
    language: Option<String>,
    http: reqwest::Client,
}

/// T-101: model-facing prefix on the inbox row carrying a transcribed
/// voice message. The constant lives next to the handler so the format
/// stays discoverable and consistent — operators reading agent inboxes
/// learn to recognize this exact string as "this came from audio."
const VOICE_INBOX_PREFIX: &str = "🎙 (transcribed voice, may have misspellings):";

impl State {
    fn manager_project(&self) -> Option<&str> {
        self.manager
            .as_deref()
            .and_then(|m| m.split_once(':').map(|(p, _)| p))
    }
}

impl State {
    fn is_authorized(&self, chat: i64) -> bool {
        self.allow.contains(&chat)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    startup::run().await
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod regression_tests;
