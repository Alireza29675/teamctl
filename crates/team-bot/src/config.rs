use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Clone)]
#[command(name = "team-bot", version, about = "Telegram interface for teamctl")]
pub(super) struct Cli {
    /// Path to the SQLite mailbox.
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    pub(super) mailbox: PathBuf,

    /// Telegram bot token.
    #[arg(long, env = "TEAMCTL_TELEGRAM_TOKEN")]
    pub(super) token: String,

    /// Comma-separated list of authorized chat ids. May be empty during
    /// bootstrap — the bot will then reply to `/start` with the caller's
    /// chat id so it can be added to `.env`.
    #[arg(long, env = "TEAMCTL_TELEGRAM_CHATS")]
    pub(super) authorized_chat_ids: Option<String>,

    /// Scope this bot to one manager. When set, it forwards only messages
    /// addressed to that manager and only surfaces approvals requested by
    /// agents in that project. Two bot instances against the same mailbox
    /// can safely coexist when each scopes to a different manager.
    ///
    /// Format: `<project>:<manager>`.
    #[arg(long, env = "TEAMCTL_MANAGER")]
    pub(super) manager: Option<String>,

    /// T-367: friendly label for the scoped manager, resolved from the
    /// agent's `display_name` (T-160) by `teamctl bot up`. Used only to make
    /// the first-connect greeting read "Connected to <name> via teamctl" with
    /// the human label instead of the bare `<project>:<manager>` id. Falls
    /// back to `--manager` when unset.
    #[arg(long, env = "TEAMCTL_MANAGER_DISPLAY_NAME")]
    pub(super) manager_display_name: Option<String>,

    /// Tmux session prefix (matches `compose.global.supervisor.tmux_prefix`).
    /// Used by slash-passthrough (T-086-G) to compute `<prefix><project>-<role>`
    /// for the manager's tmux session. `teamctl bot up` populates this from
    /// compose; the default matches `team-core`'s default prefix so a hand-
    /// launched bot still works on a stock team.
    #[arg(long, env = "TEAMCTL_TMUX_PREFIX", default_value = "t-")]
    pub(super) tmux_prefix: String,

    /// T-101 voice STT: provider arm. Currently only `groq` is supported.
    /// All four `--stt-*` flags are populated by `teamctl bot up` from
    /// `interfaces.telegram.speech_to_text` after the api_key_env var is
    /// resolved at spawn time. When any are absent, voice messages stay
    /// unhandled (preserves prior behavior).
    #[arg(long, env = "TEAMCTL_STT_PROVIDER")]
    pub(super) stt_provider: Option<String>,

    /// T-101 voice STT: provider API key (already resolved from env).
    #[arg(long, env = "TEAMCTL_STT_API_KEY")]
    pub(super) stt_api_key: Option<String>,

    /// T-101 voice STT: provider model id (e.g. `whisper-large-v3`).
    #[arg(long, env = "TEAMCTL_STT_MODEL")]
    pub(super) stt_model: Option<String>,

    /// T-101 voice STT: optional language hint forwarded to the provider.
    #[arg(long, env = "TEAMCTL_STT_LANGUAGE")]
    pub(super) stt_language: Option<String>,

    /// Rich messages are ON by default: an outbound text message *without* a
    /// reply target renders via Telegram Bot API 10.1 `sendRichMessage` (native
    /// rich markdown — tables, headings, lists, code, quotes), with a
    /// transparent fallback to the HTML sender on any failure so a message is
    /// never lost. Replies always use the HTML sender to preserve Telegram
    /// threading. Pass `--no-rich-messages` (or set
    /// `TEAMCTL_TELEGRAM_NO_RICH=true`) to force the plain HTML sender for every
    /// message; the env var also accepts `1`/`0`/`yes`/`no`.
    // BoolishValueParser makes the env accept 1/0/yes/no — a bare `bool` + `env`
    // rejects `=1` and crashes the bot at startup. clap still renders
    // `[possible values: true, false]` in --help (derived from the bool type);
    // that's cosmetically narrower than what's accepted, so don't trim the doc
    // above to match it.
    #[arg(
        long,
        env = "TEAMCTL_TELEGRAM_NO_RICH",
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        value_parser = clap::builder::BoolishValueParser::new(),
    )]
    pub(super) no_rich_messages: bool,
}

pub(super) fn parse_authorized_chats(raw: Option<&str>) -> Result<Vec<i64>> {
    raw.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse()
                .with_context(|| format!("invalid authorized chat id: {part}"))
        })
        .collect()
}
