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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, ChatAction, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
    MessageId, ParseMode, ReactionType, ReplyParameters,
};
use tokio::sync::Mutex;

#[derive(Parser, Clone)]
#[command(name = "team-bot", version, about = "Telegram interface for teamctl")]
struct Cli {
    /// Path to the SQLite mailbox.
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    mailbox: PathBuf,

    /// Telegram bot token.
    #[arg(long, env = "TEAMCTL_TELEGRAM_TOKEN")]
    token: String,

    /// Comma-separated list of authorized chat ids. May be empty during
    /// bootstrap — the bot will then reply to `/start` with the caller's
    /// chat id so it can be added to `.env`.
    #[arg(long, env = "TEAMCTL_TELEGRAM_CHATS")]
    authorized_chat_ids: Option<String>,

    /// Scope this bot to one manager. When set, it forwards only messages
    /// addressed to that manager and only surfaces approvals requested by
    /// agents in that project. Two bot instances against the same mailbox
    /// can safely coexist when each scopes to a different manager.
    ///
    /// Format: `<project>:<manager>`.
    #[arg(long, env = "TEAMCTL_MANAGER")]
    manager: Option<String>,

    /// T-367: friendly label for the scoped manager, resolved from the
    /// agent's `display_name` (T-160) by `teamctl bot up`. Used only to make
    /// the first-connect greeting read "Connected to <name> via teamctl" with
    /// the human label instead of the bare `<project>:<manager>` id. Falls
    /// back to `--manager` when unset.
    #[arg(long, env = "TEAMCTL_MANAGER_DISPLAY_NAME")]
    manager_display_name: Option<String>,

    /// Tmux session prefix (matches `compose.global.supervisor.tmux_prefix`).
    /// Used by slash-passthrough (T-086-G) to compute `<prefix><project>-<role>`
    /// for the manager's tmux session. `teamctl bot up` populates this from
    /// compose; the default matches `team-core`'s default prefix so a hand-
    /// launched bot still works on a stock team.
    #[arg(long, env = "TEAMCTL_TMUX_PREFIX", default_value = "t-")]
    tmux_prefix: String,

    /// T-101 voice STT: provider arm. Currently only `groq` is supported.
    /// All four `--stt-*` flags are populated by `teamctl bot up` from
    /// `interfaces.telegram.speech_to_text` after the api_key_env var is
    /// resolved at spawn time. When any are absent, voice messages stay
    /// unhandled (preserves prior behavior).
    #[arg(long, env = "TEAMCTL_STT_PROVIDER")]
    stt_provider: Option<String>,

    /// T-101 voice STT: provider API key (already resolved from env).
    #[arg(long, env = "TEAMCTL_STT_API_KEY")]
    stt_api_key: Option<String>,

    /// T-101 voice STT: provider model id (e.g. `whisper-large-v3`).
    #[arg(long, env = "TEAMCTL_STT_MODEL")]
    stt_model: Option<String>,

    /// T-101 voice STT: optional language hint forwarded to the provider.
    #[arg(long, env = "TEAMCTL_STT_LANGUAGE")]
    stt_language: Option<String>,
}

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
        self.allow.is_empty() || self.allow.contains(&chat)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_BOT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let bot = Bot::new(&cli.token);
    let conn = open_mailbox(&cli.mailbox)?;
    let allow: Vec<i64> = cli
        .authorized_chat_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    let media_root = cli
        .mailbox
        .parent()
        .map(|p| p.join("inbound-media"))
        .unwrap_or_else(|| PathBuf::from("inbound-media"));
    // T-101: build the STT runtime only when all three required flags
    // (provider, key, model) are present. Partial config is treated as
    // "no voice" rather than an error — `teamctl bot up` already prints a
    // skip line when the API key env var is unset.
    let stt = match (cli.stt_provider, cli.stt_api_key, cli.stt_model) {
        (Some(provider), Some(api_key), Some(model)) => Some(SttRuntime {
            provider,
            api_key,
            model,
            language: cli.stt_language,
            http: reqwest::Client::new(),
        }),
        _ => None,
    };
    let state = Arc::new(State {
        conn: Mutex::new(conn),
        allow,
        manager: cli.manager,
        manager_display_name: cli.manager_display_name,
        tmux_prefix: cli.tmux_prefix,
        media_root,
        stt,
        typing: Mutex::new(HashMap::new()),
    });

    // T-086-H: register the manager's runtime-appropriate slash commands
    // with Telegram so the operator gets autocomplete on `/`. Manager-scoped
    // CC bots register the curated `CC_SLASH_COMMANDS` list; non-CC and
    // unscoped bots register nothing (clean degrade per Decision 6). The
    // registration is best-effort — a Telegram API error is logged but
    // doesn't abort startup, since slash-passthrough (PR-G) still works
    // when the operator types the chord manually.
    let runtime = if let Some(mgr) = state.manager.as_deref() {
        let c = state.conn.lock().await;
        agent_runtime(&c, mgr)
    } else {
        None
    };
    let commands = commands_for_runtime(runtime.as_deref());
    if !commands.is_empty() {
        if let Err(e) = bot.set_my_commands(commands).await {
            tracing::warn!(
                "set_my_commands failed (operator gets no autocomplete; \
                 slash-passthrough still works manually): {e}"
            );
        }
    }

    // Outbound: poll approvals + mailbox, surface to primary chat.
    {
        let bot = bot.clone();
        let state = state.clone();
        tokio::spawn(async move { outbound_loop(bot, state).await });
    }

    // T-102 typing indicator: re-fire `sendChatAction` every ~4s for any
    // chat with a still-active typing window. Kept as its own task
    // because the outbound poll cadence (500ms) is too tight to drive
    // refreshes from there without churning Telegram with redundant
    // calls.
    {
        let bot = bot.clone();
        let state = state.clone();
        tokio::spawn(async move { typing_refresh_loop(bot, state).await });
    }

    // Inbound: teloxide repl-style, one handler for everything.
    let bot_inbound = bot.clone();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint({
            let state = state.clone();
            move |bot: Bot, msg: Message| {
                let state = state.clone();
                async move { handle_message(bot, msg, state).await }
            }
        }))
        .branch(Update::filter_callback_query().endpoint({
            let state = state.clone();
            move |bot: Bot, q: CallbackQuery| {
                let state = state.clone();
                async move { handle_callback(bot, q, state).await }
            }
        }));

    Dispatcher::builder(bot_inbound, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    Ok(())
}

/// T-104: detect a leading `/readnow ` prefix on a Telegram body and split
/// it from the routable text. Returns `(text, delivery_mode)` where
/// `delivery_mode` is `Some("immediate")` iff the prefix matched (case-
/// sensitive, single-space separator) and `None` otherwise. Empty body
/// after the prefix still returns `Some("immediate")` so the caller can
/// decide how to handle the empty-payload case.
fn peel_readnow(body: &str) -> (&str, Option<&'static str>) {
    if let Some(rest) = body.strip_prefix("/readnow ") {
        (rest, Some("immediate"))
    } else {
        (body, None)
    }
}

fn open_mailbox(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path).context("open mailbox")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;
    Ok(conn)
}

async fn handle_message(bot: Bot, msg: Message, state: Arc<State>) -> ResponseResult<()> {
    let chat_id = msg.chat.id.0;
    let trimmed = msg.text().map(str::trim).unwrap_or("");

    // Bootstrap: a chat that isn't on the allow list gets a one-shot reply
    // to `/start` exposing its own chat id, so the operator can paste it
    // into `.env` without hunting for @userinfobot.
    if !state.allow.contains(&chat_id) && trimmed == "/start" {
        bot.send_message(
            msg.chat.id,
            format!(
                "This chat isn't authorized yet.\n\n\
                 Your chat id: {chat_id}\n\n\
                 Add it to .env next to your team-compose.yaml:\n\
                 TEAMCTL_TELEGRAM_CHATS={chat_id}\n\n\
                 Then restart team-bot."
            ),
        )
        .await?;
        return Ok(());
    }

    if !state.is_authorized(chat_id) {
        return Ok(());
    }
    // T-086-C inbound media: photos and documents arrive with `msg.text()`
    // empty (the caption sits on `msg.caption()` instead). Detect before the
    // text-routing chain — without this, media messages would silently fall
    // through every arm and the operator would see no acknowledgement.
    if msg.photo().is_some() || msg.document().is_some() {
        return handle_inbound_media(&bot, &msg, &state).await;
    }
    // T-101: inbound voice note. Detect before the text-routing chain since
    // `msg.text()` is empty on a voice message — without this, voice would
    // silently fall through every arm. Voice handling requires both an STT
    // runtime and a manager-scoped bot (we need someone to route the
    // transcript to). Either missing → fall through (preserves prior
    // behavior: voice messages stay unhandled).
    if msg.voice().is_some() && state.stt.is_some() && state.manager.is_some() {
        return handle_voice(&bot, &msg, &state).await;
    }
    // T-236: voice arrived but STT isn't configured on this manager-scoped
    // bot. Reply with a config hint rather than silently dropping —
    // operator otherwise can't tell whether the voice was received,
    // intentionally ignored, or their config is broken. Unscoped bots
    // (`state.manager.is_none()`) keep today's silent fall-through; their
    // inbound handling is intentionally minimal and out of scope here.
    if msg.voice().is_some() && state.stt.is_none() && state.manager.is_some() {
        return handle_voice_stt_missing(&bot, &msg).await;
    }
    // Capture the inbound Telegram message id on every mailbox row we
    // write. T-086-B feeds it to `react_to_user.telegram_msg_id` for
    // emoji reactions. T-168 also has the store look it up server-side
    // when an agent's `reply_to_user.reply_to_message_id` (= a mailbox
    // id) references this row, resolving to the value persisted here.
    let inbound_msg_id: i64 = msg.id.0 as i64;
    if let Some(rest) = trimmed.strip_prefix("/dm ") {
        if let Some((target, body)) = rest.split_once(' ') {
            if let Some((project, _)) = target.split_once(':') {
                // T-104: `/readnow ` on the body bypasses lazy delivery so
                // the message lands inline in the agent's input stream
                // instead of as a stub. Single-space-separated, case-
                // sensitive prefix; stripped before insert.
                let (body, delivery_mode) = peel_readnow(body);
                let c = state.conn.lock().await;
                let _ = c.execute(
                    "INSERT INTO messages
                        (project_id, sender, recipient, text, sent_at, telegram_msg_id, delivery_mode)
                     VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'), ?4, ?5)",
                    params![project, target, body, inbound_msg_id, delivery_mode],
                );
                drop(c);
                bot.send_message(msg.chat.id, format!("→ {target}")).await?;
            }
        }
    } else if !trimmed.is_empty() && !trimmed.starts_with('/') && state.manager.is_some() {
        // Plain text on a manager-scoped bot: route the message to the
        // bot's manager. The whole point of `teamctl bot setup`'s 1:1
        // mapping is that DMing the bot reaches the matching manager
        // without `/dm role text` ceremony.
        let target = state.manager.as_deref().unwrap();
        if let Some((project, _)) = target.split_once(':') {
            let c = state.conn.lock().await;
            let _ = c.execute(
                "INSERT INTO messages
                    (project_id, sender, recipient, text, sent_at, telegram_msg_id)
                 VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'), ?4)",
                params![project, target, trimmed, inbound_msg_id],
            );
            drop(c);
            bot.send_message(msg.chat.id, format!("→ {target}")).await?;
        }
    } else if trimmed.starts_with("/readnow ") && state.manager.is_some() {
        // T-104: plain-text `/readnow ` on a manager-scoped bot. Strip the
        // prefix and route to the manager with `delivery_mode='immediate'`
        // so the body lands inline in the agent's context rather than as a
        // stub. Sits as its own arm (not folded into the plain-text arm
        // above) because Telegram's command parser treats anything starting
        // with `/` as a slash command.
        let target = state.manager.as_deref().unwrap();
        let (body, delivery_mode) = peel_readnow(trimmed);
        if !body.is_empty() {
            if let Some((project, _)) = target.split_once(':') {
                let c = state.conn.lock().await;
                let _ = c.execute(
                    "INSERT INTO messages
                        (project_id, sender, recipient, text, sent_at, telegram_msg_id, delivery_mode)
                     VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'), ?4, ?5)",
                    params![project, target, body, inbound_msg_id, delivery_mode],
                );
                drop(c);
                bot.send_message(msg.chat.id, format!("→ {target} (now)"))
                    .await?;
            }
        }
    } else if trimmed == "/pending" {
        let c = state.conn.lock().await;
        let rows: Vec<(i64, String, String, String)> = {
            let mut stmt = c
                .prepare(
                    "SELECT id, agent_id, action, summary FROM approvals WHERE status='pending' ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .flatten()
                .collect()
        };
        drop(c);
        if rows.is_empty() {
            bot.send_message(msg.chat.id, "No pending approvals.")
                .await?;
        } else {
            let mut out = String::from("Pending approvals:\n");
            for (id, agent, action, summary) in rows {
                out.push_str(&format!(
                    "#{id} {} · {}: {}\n",
                    html_escape_str(&agent),
                    html_escape_str(&action),
                    render_html(&summary),
                ));
            }
            bot.send_message(msg.chat.id, out)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    } else if trimmed == "/start" || trimmed == "/help" {
        // T-367: keep the first-contact greeting a short one-liner — just
        // confirm who you're talking to. The power-user commands (/pending,
        // /dm, slash-passthrough) still work; they live on `/help` now
        // instead of crowding the greeting. Both commands are matched here,
        // ahead of the slash-passthrough arm below, so `/help` never gets
        // typed into the manager's tmux session.
        let name = state
            .manager
            .as_deref()
            .map(|mgr| state.manager_display_name.as_deref().unwrap_or(mgr));
        let body = start_help_body(trimmed == "/help", name);
        bot.send_message(msg.chat.id, body).await?;
    } else if trimmed.starts_with('/') && state.manager.is_some() {
        // T-086-G slash-passthrough: any unrecognised slash command on a
        // manager-scoped bot gets typed straight into the manager's tmux
        // session via `tmux send-keys`. Feature-gated on `runtime: claude-code`
        // per Decision 6 (manager-only routing). Trust posture is "operator
        // owns the bot" per Decision 7 — no allowlist on slash content; the
        // bot is per-operator and chat-id-gated, the trust boundary is the
        // same as the operator's existing `tmux attach` access.
        let manager = state.manager.as_deref().unwrap();
        let runtime_opt = {
            let c = state.conn.lock().await;
            agent_runtime(&c, manager)
        };
        let Some(runtime) = runtime_opt else {
            bot.send_message(
                msg.chat.id,
                format!("unknown manager `{manager}` — slash-passthrough aborted"),
            )
            .await?;
            return Ok(());
        };
        match slash_outcome(manager, &runtime, &state.tmux_prefix) {
            SlashOutcome::Passthrough { session } => match tmux_send_keys(&session, trimmed) {
                Ok(()) => {
                    bot.send_message(msg.chat.id, format!("→ {manager}"))
                        .await?;
                }
                Err(err) => {
                    bot.send_message(msg.chat.id, format!("tmux error: {err}"))
                        .await?;
                }
            },
            SlashOutcome::Reject { reason } => {
                bot.send_message(msg.chat.id, reason).await?;
            }
        }
    }
    Ok(())
}

fn approval_outcome_line(approved: bool, approver_first_name: &str) -> String {
    let verb = if approved {
        "✅ Approved"
    } else {
        "❌ Rejected"
    };
    format!("{verb} by {approver_first_name}")
}

/// #299: outcome line for a chosen multi-option decision. Names the
/// picked label so the card reads as a record of what was decided.
fn decision_outcome_line(chosen_label: &str, approver_first_name: &str) -> String {
    format!("✅ {chosen_label} — chosen by {approver_first_name}")
}

/// #299: outcome line for the implicit Cancel. Distinct glyph + verb so
/// the operator (and the scrollback) can tell a cancel from a choice.
fn cancel_outcome_line(approver_first_name: &str) -> String {
    format!("🚫 Cancelled by {approver_first_name}")
}

/// Parsed inline-button tap. `Approve`/`Deny` are the binary
/// back-compat card; `Opt(idx)`/`Cancel` are the #299 multi-option card.
#[derive(Debug, PartialEq, Eq)]
enum CbAction {
    Approve,
    Deny,
    Opt(usize),
    Cancel,
}

/// Parse `callback_data` into `(approval_id, action)`. Formats:
/// `approve:{id}` / `deny:{id}` (binary), `opt:{id}:{idx}` /
/// `cancel:{id}` (multi-option). Returns `None` on anything malformed or
/// carrying trailing junk — `handle_callback` then ignores the tap, same
/// as the prior behavior for unrecognized data.
fn parse_callback(data: &str) -> Option<(i64, CbAction)> {
    let mut it = data.split(':');
    let verb = it.next()?;
    let id: i64 = it.next()?.parse().ok()?;
    let action = match verb {
        "approve" => CbAction::Approve,
        "deny" => CbAction::Deny,
        "cancel" => CbAction::Cancel,
        "opt" => CbAction::Opt(it.next()?.parse().ok()?),
        _ => return None,
    };
    if it.next().is_some() {
        return None;
    }
    Some((id, action))
}

/// Decode the `options_json` column into `(label, value)` pairs. NULL or
/// malformed JSON yields empty → the card falls back to the binary
/// Approve/Deny render rather than failing the send (defensive: a
/// corrupt row should degrade, not wedge the outbound loop).
fn decode_options(options_json: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = options_json else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .map(|arr| {
            arr.into_iter()
                .filter_map(|o| {
                    Some((
                        o.get("label")?.as_str()?.to_string(),
                        o.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the inline keyboard for an approval card. Empty `options` → the
/// binary Approve/Deny card (back-compat, byte-identical to the prior
/// render). Non-empty → one button per option (one per row; labels can
/// be long) followed by a Cancel row.
fn approval_keyboard(id: i64, options: &[(String, String)]) -> InlineKeyboardMarkup {
    if options.is_empty() {
        return InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("Approve", format!("approve:{id}")),
            InlineKeyboardButton::callback("Deny", format!("deny:{id}")),
        ]]);
    }
    let mut rows: Vec<Vec<InlineKeyboardButton>> = options
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            vec![InlineKeyboardButton::callback(
                label.clone(),
                format!("opt:{id}:{i}"),
            )]
        })
        .collect();
    rows.push(vec![InlineKeyboardButton::callback(
        "Cancel",
        format!("cancel:{id}"),
    )]);
    InlineKeyboardMarkup::new(rows)
}

async fn handle_callback(bot: Bot, q: CallbackQuery, state: Arc<State>) -> ResponseResult<()> {
    let chat_id = q.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    if !state.is_authorized(chat_id) {
        return Ok(());
    }
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some((id, action)) = parse_callback(&data) else {
        return Ok(());
    };

    // Resolve the tap to (terminal status, chosen value, outcome line,
    // toast glyph). `Opt` needs the immutable `options_json` from the row
    // to map idx→{label,value}; read it before the decision UPDATE (the
    // atomic WHERE status='pending' still guards against a concurrent
    // tap deciding between this read and the write).
    let (status, value, outcome, toast): (&str, Option<String>, String, String) = match action {
        CbAction::Approve => (
            "approved",
            None,
            approval_outcome_line(true, &q.from.first_name),
            format!("✅ #{id}"),
        ),
        CbAction::Deny => (
            "denied",
            None,
            approval_outcome_line(false, &q.from.first_name),
            format!("❌ #{id}"),
        ),
        CbAction::Cancel => (
            "denied",
            None,
            cancel_outcome_line(&q.from.first_name),
            format!("🚫 #{id}"),
        ),
        CbAction::Opt(idx) => {
            let opts = {
                let c = state.conn.lock().await;
                c.query_row(
                    "SELECT options_json FROM approvals WHERE id=?1",
                    params![id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
            };
            let decoded = decode_options(opts.as_deref());
            let Some((label, val)) = decoded.get(idx).cloned() else {
                // Out-of-range / corrupt options: don't decide, just
                // toast. The card stays tappable for a valid option.
                bot.answer_callback_query(q.id)
                    .text(format!("#{id} option unavailable"))
                    .await?;
                return Ok(());
            };
            (
                "decided",
                Some(val),
                decision_outcome_line(&label, &q.from.first_name),
                format!("✅ #{id}"),
            )
        }
    };

    // Atomic decision: only update if still pending. Returned row count tells
    // us whether this tap was the live decision or a stale duplicate.
    //
    // Order matters: status pin first, delivered_at flip second and
    // *only* when the status pin succeeded. The reverse order — flip
    // delivered_at unconditionally, then try the status pin — would
    // break the invariant `undeliverable ↔ delivered_at IS NULL` on
    // stale taps against rows that gc already moved to undeliverable.
    let decided_now = {
        let c = state.conn.lock().await;
        let n = c
            .execute(
                "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'), decided_by='user:telegram', decision_value=?2
                 WHERE id=?3 AND status='pending'",
                params![status, value, id],
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if n {
            let _ = c.execute(
                "UPDATE approvals SET delivered_at=strftime('%s','now')
                 WHERE id=?1 AND delivered_at IS NULL",
                params![id],
            );
        }
        n
    };

    if !decided_now {
        // Stale tap: row already terminal. Friendly toast, leave the message.
        bot.answer_callback_query(q.id)
            .text(format!("#{id} already resolved"))
            .await?;
        return Ok(());
    }

    // Live decision: edit the original message in-place to (a) append the
    // outcome line and (b) drop the inline buttons so the card can't be
    // re-clicked.
    if let Some(msg) = q.message.as_ref() {
        let chat = msg.chat().id;
        let mid = msg.id();
        let original = msg.regular_message().and_then(|m| m.text()).unwrap_or("");
        let new_text = if original.is_empty() {
            outcome.clone()
        } else {
            format!("{original}\n\n{outcome}")
        };
        let _ = bot.edit_message_text(chat, mid, new_text).await;
        let _ = bot
            .edit_message_reply_markup(chat, mid)
            .reply_markup(InlineKeyboardMarkup::new(Vec::<Vec<_>>::new()))
            .await;
    }

    bot.answer_callback_query(q.id).text(toast).await?;
    Ok(())
}

async fn outbound_loop(bot: Bot, state: Arc<State>) {
    let Some(&primary) = state.allow.first() else {
        tracing::warn!("no authorized_chat_ids — outbound disabled");
        return;
    };
    let chat = ChatId(primary);
    let mut last_approval_id: i64 = current_max(&state, "approvals").await;
    let mut last_msg_id: i64 = current_max(&state, "messages").await;

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Project-scope filter only — manager-level routing happens in Rust
        // below so that scoped bots only surface approvals filed by agents
        // that roll up to *their* manager (T-027 single-channel).
        type ApprovalRow = (i64, String, String, String, Option<String>);
        let approvals: Vec<ApprovalRow> = {
            let c = state.conn.lock().await;
            let rows: Vec<ApprovalRow> = match state.manager_project() {
                Some(project) => {
                    let mut stmt = c
                        .prepare(
                            "SELECT id, agent_id, action, summary, options_json FROM approvals
                             WHERE status='pending' AND id > ?1 AND project_id = ?2
                             ORDER BY id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_approval_id, project], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
                None => {
                    let mut stmt = c
                        .prepare(
                            "SELECT id, agent_id, action, summary, options_json FROM approvals
                             WHERE status='pending' AND id > ?1 ORDER BY id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_approval_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
            };
            rows
        };
        for (id, agent, action, summary, options_json) in approvals {
            last_approval_id = last_approval_id.max(id);
            // T-027: when scoped to a manager, only surface approvals filed by
            // agents that report up to *this* bot's manager. With a manager
            // bot per tier (eng_lead, pm) Alireza sees one prompt per agent.
            // Unscoped bots take the back-compat path (route everything).
            let route_ok = {
                let c = state.conn.lock().await;
                should_route(state.manager.as_deref(), &agent, &c)
            };
            if !route_ok {
                continue;
            }
            // #299: NULL options_json → binary Approve/Deny (byte-
            // identical to the prior render); a decoded option list →
            // one button per option + an implicit Cancel.
            let kb = approval_keyboard(id, &decode_options(options_json.as_deref()));
            // T-140: parity with /pending — escape interpolated agent
            // payloads even when today's `[a-z0-9_-]:[a-z0-9_-]` schema
            // makes `<>&` unreachable. Defense-in-depth: the renderer
            // doesn't lean on the schema invariant.
            let text = format!(
                "🔐 #{id}  {}\naction: {}\n{}",
                html_escape_str(&agent),
                html_escape_str(&action),
                render_html(&summary),
            );
            let send_ok = bot
                .send_message(chat, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(kb)
                .await
                .is_ok();
            if send_ok {
                let c = state.conn.lock().await;
                let _ = c.execute(
                    "UPDATE approvals SET delivered_at=strftime('%s','now')
                     WHERE id=?1 AND delivered_at IS NULL",
                    params![id],
                );
            }
        }

        // Forward replies addressed to the human. The agent-side `reply_to_user`
        // tool inserts rows with `recipient = 'user:telegram'`. Project-scope
        // is the SQL pre-filter; manager-level routing happens in Rust below
        // via `should_route` so multiple bots in the same project (one per
        // manager) don't fan out the same reply.
        //
        // T-086-A: rows now carry `kind` + `structured_payload` for image and
        // file content. NULL `kind` means text (legacy callers + the
        // text-only `reply_to_user` path), preserving back-compat against
        // older databases without a forced migration.
        let forwardable: Vec<MailboxRow> = {
            let c = state.conn.lock().await;
            let rows: Vec<MailboxRow> = match state.manager_project() {
                Some(project) => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.text, m.kind, m.structured_payload,
                                    m.telegram_msg_id
                             FROM messages m
                             WHERE m.id > ?1
                               AND m.recipient = 'user:telegram'
                               AND m.acked_at IS NULL
                               AND m.project_id = ?2
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id, project], MailboxRow::from_row)
                        .unwrap()
                        .flatten()
                        .collect()
                }
                None => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.text, m.kind, m.structured_payload,
                                    m.telegram_msg_id
                             FROM messages m
                             WHERE m.id > ?1
                               AND m.recipient = 'user:telegram'
                               AND m.acked_at IS NULL
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id], MailboxRow::from_row)
                        .unwrap()
                        .flatten()
                        .collect()
                }
            };
            rows
        };
        for row in forwardable {
            last_msg_id = last_msg_id.max(row.id);
            // Per-manager scoping: only forward replies whose sender rolls up
            // to *this* bot's manager. Without this, every bot in the project
            // forwarded every reply (e.g. eng_lead's reply landing in pm and
            // marketing chats too). Unscoped bots take the back-compat path.
            let route_ok = {
                let c = state.conn.lock().await;
                should_route(state.manager.as_deref(), &row.sender, &c)
            };
            if !route_ok {
                continue;
            }
            // T-102: keep the typing window in sync with the dispatch
            // about to happen. Text/image/file → clear (real content
            // arriving means the indicator should disappear); typing →
            // open/extend so the refresh loop keeps it alive until the
            // ceiling. Reaction + UnknownFallback don't touch the
            // window — a reaction is a soft signal, not a "the agent
            // finished talking" event.
            let kind = classify_kind(row.kind.as_deref());
            match kind {
                DispatchKind::Text | DispatchKind::Image | DispatchKind::File => {
                    let mut map = state.typing.lock().await;
                    clear_typing_window(&mut map, chat);
                }
                DispatchKind::Typing => {
                    let mut map = state.typing.lock().await;
                    extend_typing_window(&mut map, chat, Instant::now(), TYPING_WINDOW_CEILING);
                    drop(map);
                    // Fire one immediately so the operator sees
                    // "typing…" within the next ~1s rather than waiting
                    // up to TYPING_REFRESH_INTERVAL for the refresh
                    // task's first tick.
                    if let Err(e) = bot.send_chat_action(chat, ChatAction::Typing).await {
                        tracing::warn!("send_chat_action failed for row {}: {e}", row.id);
                    }
                }
                _ => {}
            }
            if !matches!(kind, DispatchKind::Typing) {
                forward_row(&bot, chat, &row).await;
            }
            let c = state.conn.lock().await;
            let _ = c.execute(
                "UPDATE messages SET acked_at = strftime('%s','now') WHERE id = ?1",
                params![row.id],
            );
        }
    }
}

/// T-102 background task: every `TYPING_REFRESH_INTERVAL`, drop expired
/// per-chat windows and re-fire `sendChatAction("typing")` on whatever
/// remains. Telegram's typing indicator persists ~5s natively, so a 4s
/// refresh keeps the bubble visible without gaps. The ceiling
/// (`TYPING_WINDOW_CEILING`) bounds the refresh — once `now` passes the
/// stored deadline the entry is dropped, the indicator naturally
/// expires on the Telegram side, and the chat goes quiet.
async fn typing_refresh_loop(bot: Bot, state: Arc<State>) {
    loop {
        tokio::time::sleep(TYPING_REFRESH_INTERVAL).await;
        let active: Vec<ChatId> = {
            let mut map = state.typing.lock().await;
            refresh_typing_windows(&mut map, Instant::now())
        };
        for chat in active {
            if let Err(e) = bot.send_chat_action(chat, ChatAction::Typing).await {
                tracing::warn!("typing refresh send_chat_action failed for {chat}: {e}");
            }
        }
    }
}

/// One mailbox row in the shape the outbound loop forwards. `kind` is `None`
/// for legacy text rows; structured kinds (image, file) carry the JSON
/// payload describing source + value + optional caption. `telegram_msg_id`
/// (T-086-B) is the Telegram message id this row should reply to — when
/// `Some`, the dispatcher attaches `reply_parameters` so the outbound
/// message visually nests under the operator's earlier message.
#[derive(Debug, Clone)]
struct MailboxRow {
    id: i64,
    sender: String,
    text: String,
    kind: Option<String>,
    payload: Option<String>,
    telegram_msg_id: Option<i64>,
}

impl MailboxRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            sender: r.get(1)?,
            text: r.get(2)?,
            kind: r.get(3)?,
            payload: r.get(4)?,
            telegram_msg_id: r.get(5)?,
        })
    }
}

/// Build a teloxide `ReplyParameters` from a stored Telegram message id, or
/// `None` when no threading is requested. Pulled out so unit tests pin the
/// presence/absence call without spinning up a real `Bot` — the `i32` cast
/// is safe because Telegram message ids stay within `i32` range.
fn reply_parameters_for(telegram_msg_id: Option<i64>) -> Option<ReplyParameters> {
    telegram_msg_id.map(|id| ReplyParameters::new(MessageId(id as i32)))
}

/// Parsed structured payload — `source` ("path"|"url"), `value` (the path or
/// URL), optional caption. `parse_payload` turns the JSON string into this
/// shape; failure cases fall back to text rendering with the raw payload
/// surfaced so the operator still sees something.
struct MediaPayload {
    source: String,
    value: String,
    caption: Option<String>,
}

fn parse_payload(payload: &str) -> Option<MediaPayload> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let source = v.get("source")?.as_str()?.to_string();
    let value = v.get("value")?.as_str()?.to_string();
    let caption = v
        .get("caption")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    Some(MediaPayload {
        source,
        value,
        caption,
    })
}

/// Build a teloxide `InputFile` from a parsed payload's source + value.
/// `path` resolves to a local file; `url` parses the value as a URL the
/// Telegram servers fetch directly.
fn input_file_from(payload: &MediaPayload) -> Option<InputFile> {
    match payload.source.as_str() {
        "path" => Some(InputFile::file(&payload.value)),
        "url" => Some(InputFile::url(payload.value.parse().ok()?)),
        _ => None,
    }
}

/// Decision the dispatcher makes for a row's `kind`. Kept as a plain enum so
/// it's testable without instantiating a teloxide `Bot`; the actual API call
/// happens in `forward_row` once the decision is made.
#[derive(Debug, PartialEq, Eq)]
enum DispatchKind {
    Text,
    Image,
    File,
    /// T-086-E: outbound reaction. Payload carries `{telegram_msg_id, emoji}`;
    /// the dispatcher routes through `setMessageReaction` rather than
    /// sending a chat message.
    Reaction,
    /// T-102: outbound typing indicator. No payload fields used — the
    /// row is purely a discriminator that tells the bot to open or
    /// extend a per-chat typing window. The dispatcher fires one
    /// `sendChatAction("typing")` immediately and leaves the periodic
    /// refresh to `typing_refresh_loop`.
    Typing,
    /// Structured row whose payload didn't parse — surface as a text
    /// fallback so the operator sees the raw payload rather than nothing.
    UnknownFallback,
}

fn classify_kind(kind: Option<&str>) -> DispatchKind {
    match kind {
        None | Some("text") | Some("") => DispatchKind::Text,
        Some("image") => DispatchKind::Image,
        Some("file") => DispatchKind::File,
        Some("reaction") => DispatchKind::Reaction,
        Some("typing") => DispatchKind::Typing,
        _ => DispatchKind::UnknownFallback,
    }
}

/// T-102 pure helper: open or extend a typing window for `chat`. The
/// new deadline is `now + ceiling`; an existing entry's deadline is
/// overwritten (this is the spec's "second call resets the 10s clock").
/// Returns the freshly written deadline so callers / tests can assert
/// against it.
fn extend_typing_window(
    map: &mut HashMap<ChatId, Instant>,
    chat: ChatId,
    now: Instant,
    ceiling: Duration,
) -> Instant {
    let deadline = now + ceiling;
    map.insert(chat, deadline);
    deadline
}

/// T-102 pure helper: clear the typing window for `chat`. Returns
/// whether an entry was actually removed; the dispatcher doesn't act on
/// the bool today, but the return value lets tests pin both the present
/// and absent cases. Called before every text/image/file dispatch so
/// the indicator disappears the moment a real message lands.
fn clear_typing_window(map: &mut HashMap<ChatId, Instant>, chat: ChatId) -> bool {
    map.remove(&chat).is_some()
}

/// T-102 pure helper: drop entries whose deadline has passed and return
/// the chats whose windows are still active at `now`. The refresh loop
/// uses the returned list to issue another `sendChatAction` round.
fn refresh_typing_windows(map: &mut HashMap<ChatId, Instant>, now: Instant) -> Vec<ChatId> {
    map.retain(|_, deadline| *deadline > now);
    map.keys().copied().collect()
}

/// Parsed reaction payload (T-086-E). The MCP layer writes
/// `{"telegram_msg_id": <i64>, "emoji": "<str>"}`; this turns it back into a
/// typed pair the dispatcher hands to `setMessageReaction`. Returns `None`
/// when either field is missing or the wrong shape — the dispatcher's
/// fallback then logs and skips rather than calling Telegram with bogus
/// args.
struct ReactionPayload {
    telegram_msg_id: i64,
    emoji: String,
}

fn parse_reaction_payload(payload: &str) -> Option<ReactionPayload> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let telegram_msg_id = v.get("telegram_msg_id")?.as_i64()?;
    let emoji = v.get("emoji")?.as_str()?.to_string();
    Some(ReactionPayload {
        telegram_msg_id,
        emoji,
    })
}

async fn forward_row(bot: &Bot, chat: ChatId, row: &MailboxRow) {
    let kind = classify_kind(row.kind.as_deref());
    // T-140: html-escape `row.sender` so the renderer doesn't lean on
    // today's agent-id schema (`[a-z0-9_-]:[a-z0-9_-]`). The em-dash and
    // literal "replied by" carry no `<>&`, so escaping the whole format
    // result would be redundant — escape just the interpolated field.
    let attribution = format!("\n\n— replied by {}", html_escape_str(&row.sender));
    let reply = reply_parameters_for(row.telegram_msg_id);
    match kind {
        DispatchKind::Text => {
            let mut req = bot
                .send_message(chat, format!("{}{attribution}", render_html(&row.text)))
                .parse_mode(ParseMode::Html);
            if let Some(rp) = reply.clone() {
                req = req.reply_parameters(rp);
            }
            if let Some(e) = req.await.err() {
                tracing::warn!("send_message (text) failed for mailbox row {}: {e}", row.id);
            }
        }
        DispatchKind::Image | DispatchKind::File => {
            let Some(payload) = row.payload.as_deref().and_then(parse_payload) else {
                if let Some(e) = bot
                    .send_message(
                        chat,
                        format!(
                            "{} (media payload unparseable){attribution}",
                            render_html(&row.text)
                        ),
                    )
                    .parse_mode(ParseMode::Html)
                    .await
                    .err()
                {
                    tracing::warn!(
                        "send_message (media-unparseable fallback) failed for mailbox row {}: {e}",
                        row.id
                    );
                }
                return;
            };
            let Some(input) = input_file_from(&payload) else {
                if let Some(e) = bot
                    .send_message(
                        chat,
                        format!(
                            "{} (unsupported media source <code>{}</code>){attribution}",
                            render_html(&row.text),
                            html_escape_str(&payload.source)
                        ),
                    )
                    .parse_mode(ParseMode::Html)
                    .await
                    .err()
                {
                    tracing::warn!(
                        "send_message (unsupported-source fallback) failed for mailbox row {}: {e}",
                        row.id
                    );
                }
                return;
            };
            let caption_text = payload
                .caption
                .as_deref()
                .map(|c| format!("{}{attribution}", render_html(c)))
                .unwrap_or_else(|| attribution.trim_start().to_string());
            let result = match kind {
                DispatchKind::Image => {
                    let mut req = bot
                        .send_photo(chat, input)
                        .caption(caption_text)
                        .parse_mode(ParseMode::Html);
                    if let Some(rp) = reply.clone() {
                        req = req.reply_parameters(rp);
                    }
                    req.await.err()
                }
                DispatchKind::File => {
                    let mut req = bot
                        .send_document(chat, input)
                        .caption(caption_text)
                        .parse_mode(ParseMode::Html);
                    if let Some(rp) = reply.clone() {
                        req = req.reply_parameters(rp);
                    }
                    req.await.err()
                }
                _ => unreachable!(),
            };
            if let Some(e) = result {
                tracing::warn!(
                    "send_{} failed for mailbox row {}: {e}",
                    if kind == DispatchKind::Image {
                        "photo"
                    } else {
                        "document"
                    },
                    row.id
                );
            }
        }
        DispatchKind::Reaction => {
            // T-086-E: reactions ride the existing kind discriminator; the
            // dispatcher routes through `setMessageReaction` instead of a
            // send-message call. Failure-mode (unparseable payload) logs +
            // skips — no operator-visible chat noise, since a reaction is
            // a soft signal anyway. Telegram-side rejection (not in chat,
            // emoji disallowed, etc.) bubbles up via `tracing::warn!`.
            let Some(reaction) = row.payload.as_deref().and_then(parse_reaction_payload) else {
                tracing::warn!(
                    "reaction payload unparseable for mailbox row {} (skipping)",
                    row.id
                );
                return;
            };
            let result = bot
                .set_message_reaction(chat, MessageId(reaction.telegram_msg_id as i32))
                .reaction(vec![ReactionType::Emoji {
                    emoji: reaction.emoji,
                }])
                .await
                .err();
            if let Some(e) = result {
                tracing::warn!("set_message_reaction failed for row {}: {e}", row.id);
            }
        }
        DispatchKind::Typing => {
            // T-102: typing rows are handled by `outbound_loop` directly
            // (it opens/extends the per-chat window and fires
            // sendChatAction). They short-circuit before reaching
            // `forward_row`; this arm exists only to keep the match
            // exhaustive without flagging the row as unknown.
        }
        DispatchKind::UnknownFallback => {
            if let Some(e) = bot
                .send_message(chat, format!("{}{attribution}", render_html(&row.text)))
                .parse_mode(ParseMode::Html)
                .await
                .err()
            {
                tracing::warn!(
                    "send_message (unknown-kind fallback) failed for mailbox row {}: {e}",
                    row.id
                );
            }
        }
    }
}

async fn current_max(state: &Arc<State>, table: &str) -> i64 {
    let sql = format!("SELECT COALESCE(MAX(id), 0) FROM {table}");
    let c = state.conn.lock().await;
    c.query_row(&sql, [], |r| r.get(0)).unwrap_or(0)
}

/// Resolve the `<project>:<manager>` an agent rolls up to, used by T-027 to
/// route an approval to exactly one Telegram bot. Managers report to themselves
/// (no walk needed); non-managers resolve via `agents.reports_to`. Returns
/// `None` if the agent isn't registered.
fn manager_of(conn: &Connection, agent_id: &str) -> Option<String> {
    let row: Option<(String, i64, Option<String>)> = conn
        .query_row(
            "SELECT project_id, is_manager, reports_to FROM agents WHERE id = ?1",
            params![agent_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let (project, is_manager, reports_to) = row?;
    if is_manager == 1 {
        return Some(agent_id.to_string());
    }
    let role = reports_to?;
    Some(format!("{project}:{role}"))
}

/// T-367: build the body for `/start` (greeting) or `/help` (command list).
///
/// `is_help` selects the command list; otherwise a short one-line greeting.
/// `name` is the label for a manager-scoped bot — the manager's
/// `display_name` (T-160) when set, else the `<project>:<manager>` id — and
/// `None` for an unscoped bot, which has no single manager to name. Pulled
/// out as a free function so the copy is unit-testable without a teloxide
/// `Bot` or an async runtime.
fn start_help_body(is_help: bool, name: Option<&str>) -> String {
    match (is_help, name) {
        (true, Some(name)) => format!(
            "teamctl commands:\n\
             /pending — show pending approvals\n\
             /dm <project>:<agent> <text> — send to a different agent (rare)\n\
             /<cmd> — slash-passthrough to {name}'s tmux session (Claude Code only)\n\
             \n\
             Just type a message to chat with {name}."
        ),
        (true, None) => "teamctl commands:\n\
             /dm <project>:<agent> <message> — send a DM\n\
             /pending — show pending approvals"
            .into(),
        (false, Some(name)) => format!("Connected to {name} via teamctl. Just type to chat."),
        (false, None) => "Connected via teamctl. Send /help for commands.".into(),
    }
}

/// Route an approval row to *this* bot iff:
/// - `scoped` is `None` (unscoped bot — back-compat fallback for setups
///   that predate per-manager scoping; surface every approval), or
/// - `scoped` is `Some(<project>:<manager>)` and the agent that filed
///   the approval rolls up to that manager (per `manager_of`).
///
/// Pulled out as a free function so the unscoped-vs-scoped semantics
/// are unit-testable without spinning up an async tokio runtime.
fn should_route(scoped: Option<&str>, agent_id: &str, conn: &Connection) -> bool {
    let Some(scoped) = scoped else {
        return true;
    };
    let routed = manager_of(conn, agent_id).unwrap_or_else(|| agent_id.to_string());
    routed == scoped
}

/// Look up the registered runtime for an agent. Used by slash-passthrough
/// (T-086-G) to feature-gate the chord on `runtime: claude-code` and by
/// the setMyCommands registration (T-086-H) to pick the per-runtime
/// command list. Returns `None` if the agent isn't in the mailbox's
/// `agents` table.
fn agent_runtime(conn: &Connection, agent_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT runtime FROM agents WHERE id = ?1",
        params![agent_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Decision returned by `slash_outcome` — either we have a tmux session to
/// type the slash command into, or a user-facing rejection message.
#[derive(Debug, PartialEq, Eq)]
enum SlashOutcome {
    Passthrough { session: String },
    Reject { reason: String },
}

/// Pure decision: given the manager id (`<project>:<role>`), the manager's
/// runtime, and the configured tmux prefix, decide whether slash-passthrough
/// fires and against which tmux session. Non-Claude-Code runtimes are
/// rejected per Decision 6 (manager-only / CC-only routing); the rejection
/// message names the actual runtime so the operator sees why.
fn slash_outcome(manager: &str, runtime: &str, tmux_prefix: &str) -> SlashOutcome {
    if runtime != "claude-code" {
        return SlashOutcome::Reject {
            reason: format!(
                "slash-passthrough is only supported on Claude Code agents \
                 (this manager runs `{runtime}`)."
            ),
        };
    }
    let (project, role) = match manager.split_once(':') {
        Some((p, r)) => (p, r),
        None => {
            return SlashOutcome::Reject {
                reason: format!("malformed manager id `{manager}` (expected `project:role`)."),
            };
        }
    };
    SlashOutcome::Passthrough {
        session: format!("{tmux_prefix}{project}-{role}"),
    }
}

/// Argv for the tmux send-keys invocation. Pulled out so unit tests pin the
/// exact arg shape without spinning up tmux. The literal `Enter` keyword is
/// what tells tmux to fire a Return after the body, which is what triggers
/// the Claude Code prompt to actually process the slash command.
fn tmux_send_keys_argv<'a>(session: &'a str, body: &'a str) -> [&'a str; 5] {
    ["send-keys", "-t", session, body, "Enter"]
}

/// Real-world tmux send-keys wrapper. On failure, returns the verbatim error
/// (R12 family — surface the cause to the operator rather than silent drop).
fn tmux_send_keys(session: &str, body: &str) -> Result<(), String> {
    let argv = tmux_send_keys_argv(session, body);
    let output = Command::new("tmux")
        .args(argv)
        .output()
        .map_err(|e| format!("invoke tmux: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            return Err(format!("tmux exit {}", output.status));
        }
        return Err(format!("tmux exit {}: {trimmed}", output.status));
    }
    Ok(())
}

/// Curated subset of Claude Code slash commands surfaced via Telegram's
/// `setMyCommands` API (T-086-H). Telegram restricts the `command` field to
/// lowercase letters, digits, and underscores — the hyphenated CC commands
/// (`output-style`, `pr-comments`, `release-notes`, `security-review`) are
/// excluded for that reason; operators can still type them manually and the
/// slash-passthrough lane (T-086-G) routes them to tmux just fine. Login
/// flows (`login`, `logout`, `upgrade`) are also excluded — those are
/// awkward over chat and rarely the daily-driver path.
///
/// **Maintenance note**: this list is hand-maintained on Claude Code
/// version bumps. Drift cost is bounded — the CC slash command set is
/// stable across patch releases. The dynamic-discovery alternative (parse
/// CC's `/help` output at startup) is heavier substrate for marginal gain.
/// Refresh in a polish-PR when CC ships a new minor version.
const CC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("clear", "Clear conversation history"),
    (
        "compact",
        "Compact conversation, optionally with focus instructions",
    ),
    ("cost", "Show token usage cost"),
    ("help", "Show available commands and shortcuts"),
    ("init", "Initialize a new CLAUDE.md file"),
    ("mcp", "Manage MCP servers"),
    ("model", "Set the AI model for Claude Code"),
    ("permissions", "View and edit permissions"),
    ("resume", "Resume a previous conversation"),
    ("review", "Review a pull request"),
    ("status", "Show Claude Code status"),
    ("vim", "Toggle between vim and default editing modes"),
];

/// Build the runtime-appropriate `BotCommand` list for `setMyCommands`. CC
/// managers get `CC_SLASH_COMMANDS`; everything else (codex, gemini,
/// unknown, unscoped) gets an empty list — clean degrade per Decision 6
/// (manager-only / CC-only routing). Pulled out as a free function so the
/// per-runtime mapping is unit-testable without a real Telegram bot.
fn commands_for_runtime(runtime: Option<&str>) -> Vec<BotCommand> {
    match runtime {
        Some("claude-code") => CC_SLASH_COMMANDS
            .iter()
            .map(|(c, d)| BotCommand::new(*c, *d))
            .collect(),
        _ => Vec::new(),
    }
}

// ── T-086-C inbound media ───────────────────────────────────────

/// Photo vs. document — used to label the mailbox row's `kind`. The disk
/// path naming is the same either way (`<row_id>.<ext>`), so this just
/// drives the discriminator the agent reads via `inbox_peek`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum MediaKind {
    Image,
    File,
}

/// Resolved enough information to download and record an inbound media
/// message. `file_id` feeds `bot.get_file`; `extension` + `mime` ride into
/// the structured payload so the agent can pick a vision-content shape on
/// its own runtime.
struct MediaIntent {
    file_id: String,
    extension: String,
    mime: String,
    kind: MediaKind,
}

/// Pick the largest photo size or fall back to a document; returns `None`
/// when the message carries neither (caller should defer to the text path).
/// Pulled out of `handle_inbound_media` so the picking rule is unit-testable
/// without standing up a fake `Bot`.
fn classify_media_intent(msg: &Message) -> Option<MediaIntent> {
    if let Some(photos) = msg.photo() {
        // Telegram delivers photos as a list of `PhotoSize` thumbnails — pick
        // the largest by pixel count so the agent gets the highest fidelity
        // available. Telegram's photo storage is always JPEG regardless of
        // upload format, so we hard-code the extension + mime.
        let largest = photos
            .iter()
            .max_by_key(|p| (p.width as u64).saturating_mul(p.height as u64))?;
        return Some(MediaIntent {
            file_id: largest.file.id.clone(),
            extension: "jpg".into(),
            mime: "image/jpeg".into(),
            kind: MediaKind::Image,
        });
    }
    if let Some(doc) = msg.document() {
        let mime = doc
            .mime_type
            .as_ref()
            .map(|m| m.essence_str().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let extension = extension_for_document(doc.file_name.as_deref(), &mime);
        // Telegram users often upload PNG/GIF as document (which preserves
        // the original bytes vs. the jpeg recompression of `photo`); route
        // those as Image so the agent's vision plumbing still picks them up.
        let kind = if mime.starts_with("image/") {
            MediaKind::Image
        } else {
            MediaKind::File
        };
        return Some(MediaIntent {
            file_id: doc.file.id.clone(),
            extension,
            mime,
            kind,
        });
    }
    None
}

/// Pick the file extension to use for a document upload. Prefer the
/// uploaded filename's extension when it's a clean ASCII alphanumeric
/// suffix; fall back to the mime-type lookup table otherwise. Defensive
/// against names like `report.pdf.bak` (uses `bak`) or `evil/../traversal`
/// (the rsplit_once on '.' won't match a path separator on its own, but the
/// alphanumeric guard keeps the result tame).
fn extension_for_document(filename: Option<&str>, mime: &str) -> String {
    if let Some(name) = filename {
        if let Some((_, ext)) = name.rsplit_once('.') {
            if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return ext.to_ascii_lowercase();
            }
        }
    }
    extension_from_mime(mime).into()
}

/// Mime → file-extension lookup. Covers the common Telegram-deliverable
/// shapes; everything unknown falls to `bin` so the on-disk file still
/// exists and an agent can re-mime it via libmagic if it cares.
fn extension_from_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/zip" => "zip",
        "application/json" => "json",
        _ => "bin",
    }
}

/// Compose the on-disk path for an inbound media row. `media_root` is the
/// directory configured at startup (defaults to `<mailbox-parent>/inbound-
/// media/`); the per-project subdirectory keeps a multi-project mailbox
/// from colliding row ids across projects.
fn inbound_media_path(media_root: &Path, project: &str, row_id: i64, extension: &str) -> PathBuf {
    media_root
        .join(project)
        .join(format!("{row_id}.{extension}"))
}

/// JSON shape for a successful media row. Empty captions are omitted so
/// agents reading the payload don't have to special-case the empty string.
fn media_success_payload(path: &Path, caption: &str, mime: &str, size_bytes: u64) -> String {
    let mut payload = serde_json::json!({
        "path": path.display().to_string(),
        "mime": mime,
        "size_bytes": size_bytes,
    });
    if !caption.is_empty() {
        payload["caption"] = serde_json::Value::String(caption.to_string());
    }
    payload.to_string()
}

/// JSON shape for a failed media row (R12 — no silent drops). Captures the
/// verbatim error so the agent can ack to the user with a real diagnostic.
fn media_error_payload(caption: &str, error: &str) -> String {
    let mut payload = serde_json::json!({ "error": error });
    if !caption.is_empty() {
        payload["caption"] = serde_json::Value::String(caption.to_string());
    }
    payload.to_string()
}

/// Stream a Telegram-hosted file to disk via `bot.get_file` + `bot.download_file`.
/// On any error returns a verbatim `String` so the caller can fold it into
/// the `media_error` mailbox row + the user-facing reply.
async fn download_to(bot: &Bot, file_id: &str, path: &Path, dir: &Path) -> Result<u64, String> {
    use tokio::io::AsyncWriteExt;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("create_dir_all `{}`: {e}", dir.display()))?;
    let file = bot
        .get_file(file_id)
        .await
        .map_err(|e| format!("get_file: {e}"))?;

    // #332: disk-fill counterpart of #279's RAM-OOM defense. Layer 1 —
    // fast-fail when Telegram already reports a size over the ceiling,
    // before creating any destination file on disk. Layer 2 (below) —
    // a `BoundedWriter` wrapping the file handle catches a lying-small
    // upstream that tries to stream past the ceiling mid-download.
    if (file.size as usize) > MAX_DOWNLOAD_BYTES {
        return Err(media_size_pre_reject(
            "media file",
            file.size,
            MAX_DOWNLOAD_BYTES,
        ));
    }

    let handle = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("create file `{}`: {e}", path.display()))?;
    let mut bounded = BoundedWriter::new("media file", handle, MAX_DOWNLOAD_BYTES);
    bot.download_file(&file.path, &mut bounded)
        .await
        .map_err(|e| format!("download_file: {e}"))?;

    let mut handle = bounded.into_inner();
    handle.flush().await.ok();
    drop(handle);
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("metadata: {e}"))?;
    Ok(meta.len())
}

/// Branch from `handle_message` for inbound photo/document. The two-phase
/// SQL pattern (insert placeholder → download → UPDATE on success or error)
/// keeps the row visible to the agent's `inbox_peek` from the moment the
/// message arrives — so an agent can see "media pending" while the download
/// races a slow link, rather than nothing for an unbounded stretch.
async fn handle_inbound_media(bot: &Bot, msg: &Message, state: &State) -> ResponseResult<()> {
    let Some(manager) = state.manager.as_deref() else {
        bot.send_message(
            msg.chat.id,
            "media uploads need a manager-scoped bot. \
             Run `teamctl bot up` to attach this bot to a manager.",
        )
        .await?;
        return Ok(());
    };
    let Some((project, _)) = manager.split_once(':') else {
        // `bot setup` validates this — defensive only.
        return Ok(());
    };
    let Some(intent) = classify_media_intent(msg) else {
        return Ok(());
    };
    let caption = msg.caption().unwrap_or("").to_string();

    // Insert a `media_pending` row first so we have a stable rowid to name
    // the disk file with. Caller will UPDATE the row to `image`/`file` (or
    // `media_error`) once the download resolves.
    let placeholder_payload = serde_json::json!({ "caption": caption }).to_string();
    let row_id_opt = {
        let c = state.conn.lock().await;
        match c.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, kind, structured_payload)
             VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'),
                     'media_pending', ?4)",
            params![project, manager, &caption, placeholder_payload],
        ) {
            Ok(_) => Some(c.last_insert_rowid()),
            Err(e) => {
                tracing::error!("inbound media: failed to insert placeholder row: {e}");
                None
            }
        }
    };
    let Some(row_id) = row_id_opt else {
        bot.send_message(
            msg.chat.id,
            "internal error: could not record the message; please retry.",
        )
        .await?;
        return Ok(());
    };

    let path = inbound_media_path(&state.media_root, project, row_id, &intent.extension);
    let dir = path.parent().unwrap_or(&state.media_root).to_path_buf();
    match download_to(bot, &intent.file_id, &path, &dir).await {
        Ok(size_bytes) => {
            let payload = media_success_payload(&path, &caption, &intent.mime, size_bytes);
            let kind = match intent.kind {
                MediaKind::Image => "image",
                MediaKind::File => "file",
            };
            let c = state.conn.lock().await;
            let _ = c.execute(
                "UPDATE messages SET kind = ?1, structured_payload = ?2 WHERE id = ?3",
                params![kind, payload, row_id],
            );
            drop(c);
            bot.send_message(msg.chat.id, format!("→ {manager}"))
                .await?;
        }
        Err(err) => {
            let payload = media_error_payload(&caption, &err);
            let c = state.conn.lock().await;
            let _ = c.execute(
                "UPDATE messages SET kind = 'media_error', structured_payload = ?1 WHERE id = ?2",
                params![payload, row_id],
            );
            drop(c);
            bot.send_message(msg.chat.id, format!("media download failed: {err}"))
                .await?;
        }
    }
    Ok(())
}

/// Render a small markdown subset to Telegram HTML so agent messages reach
/// the operator with formatting intact AND legitimate `_`/`*`/`` ` ``
/// characters preserved (T-134). Conservative whitelist: `**bold**`,
/// `__bold__`, `*italic*`, `` `code` ``, fenced code blocks (with optional
/// language tag — restricted to `[A-Za-z0-9_-]` per T-149, so e.g.
/// `` ```c++ `` renders with `class="language-c"`), and the existing
/// `- item` / `* item` / `+ item` → `• item` bullet glyph.
/// Single-underscore italic (`_text_`) is intentionally NOT converted
/// — underscore is too common in dev text
/// (`snake_case`, `thread_id`, URLs, paths). Inline conversion is
/// paired-only on the same line; an unmatched `*` or `` ` `` passes
/// through literally. `<`, `>`, `&` are escaped in every raw segment
/// (including inside `<code>` / `<pre>` per Telegram's HTML parser).
///
/// Callers pair the output with `.parse_mode(ParseMode::Html)` on the
/// teloxide send. Telegram supports a fixed HTML whitelist (`<b>`,
/// `<i>`, `<u>`, `<s>`, `<code>`, `<pre>`, `<a>`, `<tg-spoiler>`); the
/// emitted tags here stay inside that whitelist.
fn render_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    let lines: Vec<&str> = s.lines().collect();
    let mut i = 0;
    let mut first = true;
    while i < lines.len() {
        if !first {
            out.push('\n');
        }
        first = false;
        let line = lines[i];
        // Fenced code block? Look for a matching close on a later line.
        if let Some(lang) = fence_marker(line) {
            let close_idx = ((i + 1)..lines.len()).find(|&j| fence_marker(lines[j]).is_some());
            if let Some(close) = close_idx {
                if lang.is_empty() {
                    out.push_str("<pre>");
                } else {
                    out.push_str("<pre><code class=\"language-");
                    html_escape_into(&mut out, &lang);
                    out.push_str("\">");
                }
                for (k, body_line) in lines[(i + 1)..close].iter().enumerate() {
                    if k > 0 {
                        out.push('\n');
                    }
                    html_escape_into(&mut out, body_line);
                }
                if lang.is_empty() {
                    out.push_str("</pre>");
                } else {
                    out.push_str("</code></pre>");
                }
                i = close + 1;
                continue;
            }
            // Unmatched fence: fall through and treat as a normal line.
        }
        render_normal_line(line, &mut out);
        i += 1;
    }
    out
}

/// Return `Some(lang)` (possibly empty) if `line` is a fence marker
/// (`` ``` `` optionally followed by a language tag). Leading
/// whitespace is permitted. The language tag is parsed at the
/// boundary as the leading run of `[A-Za-z0-9_-]` characters — any
/// other byte (whitespace, quote, slash, non-ASCII, …) ends the
/// tag and the rest of the line is dropped. T-149 closes the
/// quote-in-attribute injection vector this way: a fence like
/// `` ```"x `` previously yielded `lang = "\"x"`, which then landed
/// inside `class="language-…"` and broke the parser. Schema-tighten
/// at the parse boundary instead of growing the escaper to handle
/// quotes — the ASCII-class rule also matches how every real syntax
/// highlighter (HighlightJS, Pygments, chroma) classifies a
/// language tag, so we lose no real-world capability.
fn fence_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let after = trimmed.strip_prefix("```")?;
    Some(
        after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect(),
    )
}

fn render_normal_line(line: &str, out: &mut String) {
    let trimmed = line.trim_start();
    let leading = &line[..line.len() - trimmed.len()];
    let body = if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        format!("• {rest}")
    } else {
        trimmed.to_string()
    };
    out.push_str(leading);
    render_inline_html(&body, out);
}

/// Inline-pass markdown → HTML for one line. Recognised: `**…**`,
/// `__…__` → `<b>`; `*…*` → `<i>`; `` `…` `` → `<code>`. Pairing is
/// per-line: an open delimiter without a matching close on the same
/// line falls through to escaped-literal output. Content inside a
/// recognised pair is HTML-escaped but NOT recursively re-parsed for
/// further markdown.
fn render_inline_html(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Pair lookup requires non-empty content — `<b></b>`,
        // `<i></i>`, `<code></code>` are never desired output, and the
        // empty-content match is what causes a stray `**` or backtick
        // run to swallow itself instead of passing through literally.
        if bytes.get(i..i + 2) == Some(b"**") {
            if let Some(end) = s[i + 2..].find("**").filter(|&e| e > 0) {
                let close = i + 2 + end;
                out.push_str("<b>");
                html_escape_into(out, &s[i + 2..close]);
                out.push_str("</b>");
                i = close + 2;
                continue;
            }
        }
        if bytes.get(i..i + 2) == Some(b"__") {
            if let Some(end) = s[i + 2..].find("__").filter(|&e| e > 0) {
                let close = i + 2 + end;
                out.push_str("<b>");
                html_escape_into(out, &s[i + 2..close]);
                out.push_str("</b>");
                i = close + 2;
                continue;
            }
        }
        if bytes[i] == b'`' {
            if let Some(end) = s[i + 1..].find('`').filter(|&e| e > 0) {
                let close = i + 1 + end;
                out.push_str("<code>");
                html_escape_into(out, &s[i + 1..close]);
                out.push_str("</code>");
                i = close + 1;
                continue;
            }
        }
        if bytes[i] == b'*' {
            if let Some(end) = s[i + 1..].find('*').filter(|&e| e > 0) {
                let close = i + 1 + end;
                out.push_str("<i>");
                html_escape_into(out, &s[i + 1..close]);
                out.push_str("</i>");
                i = close + 1;
                continue;
            }
        }
        let next = s[i..]
            .chars()
            .next()
            .expect("byte index inside string bounds yields a char");
        match next {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(next),
        }
        i += next.len_utf8();
    }
}

/// Escape `<`, `>`, `&` per Telegram's HTML parse mode. Quote escaping
/// is unnecessary outside attributes; the only attribute we emit is
/// `class="language-…"` and the language tag is HTML-escaped before
/// substitution.
fn html_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
}

fn html_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    html_escape_into(&mut out, s);
    out
}

// ── T-101 voice STT ────────────────────────────────────────────────────

/// Outcome of a transcription attempt. Three branches stay distinct in
/// code per the issue: a clean transcript, a "nothing recognizable"
/// signal (silence / music / noise — agent must not be disturbed), and
/// a hard failure (network, auth, provider down — surface verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SttOutcome {
    Ok(String),
    Skipped,
    Failed(String),
}

/// What the voice handler does next, derived purely from an `SttOutcome`.
/// Pulled out as data so the mapping is unit-testable without a real Bot.
#[derive(Debug, PartialEq, Eq)]
struct VoiceDecision {
    /// The Telegram reply (sent threaded under the voice message).
    user_reply: String,
    /// The mailbox row text. `None` means no inbox row — used for both
    /// `Skipped` (don't disturb the agent) and `Failed` (no garbage
    /// transcript reaching the model).
    inbox_text: Option<String>,
}

fn map_voice_outcome(outcome: &SttOutcome) -> VoiceDecision {
    match outcome {
        SttOutcome::Ok(transcript) => VoiceDecision {
            user_reply: format!("🎙 \"{transcript}\""),
            inbox_text: Some(format!("{VOICE_INBOX_PREFIX} {transcript}")),
        },
        SttOutcome::Skipped => VoiceDecision {
            user_reply: "🎙 (couldn't capture anything. did you say something? — skipping)"
                .to_string(),
            inbox_text: None,
        },
        SttOutcome::Failed(err) => VoiceDecision {
            user_reply: format!("🎙 transcription failed: {err}"),
            inbox_text: None,
        },
    }
}

/// Inbound voice handler. Caller has already verified `msg.voice()` is
/// `Some`, the bot is manager-scoped, and an `SttRuntime` is configured.
/// Mirrors `handle_inbound_media`'s shape: download → provider call →
/// branch on outcome → optionally insert mailbox row → reply (threaded).
async fn handle_voice(bot: &Bot, msg: &Message, state: &State) -> ResponseResult<()> {
    let manager = state.manager.as_deref().expect("checked by caller");
    let stt = state.stt.as_ref().expect("checked by caller");
    let Some((project, _)) = manager.split_once(':') else {
        return Ok(());
    };
    let Some(voice) = msg.voice() else {
        return Ok(());
    };
    let file_id = voice.file.id.clone();
    let inbound_msg_id: i64 = msg.id.0 as i64;
    let reply_to = ReplyParameters::new(msg.id);

    // Soft cue while the provider call is in flight. Best-effort — a
    // typing-action failure should not cancel the actual transcription.
    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;

    let audio = match download_voice_bytes(bot, &file_id).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let decision = map_voice_outcome(&SttOutcome::Failed(err));
            bot.send_message(msg.chat.id, decision.user_reply)
                .reply_parameters(reply_to)
                .await?;
            return Ok(());
        }
    };

    let outcome = transcribe(&audio, stt).await;
    let decision = map_voice_outcome(&outcome);

    if let Some(inbox_text) = decision.inbox_text.as_deref() {
        let c = state.conn.lock().await;
        // The verify-reply is about to tell the operator what was heard,
        // which raises their expectation that the agent got it. If the
        // INSERT fails we still send the reply (matches the existing
        // text/dm paths) but log loudly so the drop is diagnosable —
        // mirrors the `tracing::error!` in `handle_inbound_media`.
        if let Err(e) = c.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, telegram_msg_id)
             VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'), ?4)",
            params![project, manager, inbox_text, inbound_msg_id],
        ) {
            tracing::error!(
                "voice transcript INSERT failed for {manager}: {e} (operator was \
                 told what was heard but the agent will not receive it)"
            );
        }
    }

    bot.send_message(msg.chat.id, decision.user_reply)
        .reply_parameters(reply_to)
        .await?;
    Ok(())
}

/// T-236: body of the operator-facing reply when voice arrives on a
/// manager-scoped bot that has no `speech_to_text` runtime configured.
/// Pure function so the unit test in this file can assert the
/// done-when content (voice-glyph, cause-named, two-fix-paths, docs
/// pointer) without spinning up a Telegram mock. Plain text matches
/// the existing voice-reply parse-mode (`handle_voice` sends without
/// `.parse_mode()`); backticks render literally and operators are
/// technical enough to parse them as `code quotes`.
fn voice_stt_missing_reply() -> &'static str {
    "🎙 Voice isn't configured for this agent yet.\n\n\
     To enable, either run `/teamctl:adjust` in your project's Claude Code \
     to configure it conversationally, or add `interfaces.telegram.speech_to_text` \
     to the agent's project YAML manually.\n\n\
     Docs: https://teamctl.run/guides/telegram-bot/#voice-messages-optional"
}

/// T-236: inbound voice handler used when STT isn't configured. Caller
/// has already verified `msg.voice()` is `Some`, the bot is manager-
/// scoped, AND `state.stt.is_none()`. Mirrors `handle_voice`'s reply
/// pattern (threaded under the operator's voice message) so the
/// operator sees the response nested where they spoke.
async fn handle_voice_stt_missing(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let reply_to = ReplyParameters::new(msg.id);
    bot.send_message(msg.chat.id, voice_stt_missing_reply())
        .reply_parameters(reply_to)
        .await?;
    Ok(())
}

/// Hard upper bound on voice-file bytes accepted from Telegram.
///
/// #279: Telegram caps voice notes at 60s OGG OPUS, which lands in the
/// tens of KB to a few hundred KB in practice (≤ ~500 KB at high
/// bitrate). 2 MiB gives ~4× headroom over real-world voice notes while
/// keeping a decisive DoS bound — a malicious or buggy upstream can't
/// make the bot pre-allocate gigabytes by reporting a huge `file.size`,
/// and the streaming download is bounded too (via [`BoundedWriter`]) so
/// a lying-small upstream can't flood the `Vec` mid-download either.
const MAX_VOICE_BYTES: usize = 2 * 1024 * 1024;

/// Hard upper bound on disk-bound media downloads ([`download_to`]).
///
/// #332: same upstream-trust boundary as the voice path (`file.size`
/// is reported by Telegram, not verified), different downstream
/// threat — RAM-OOM on the voice path, **disk-fill** here. Telegram's
/// default bot-API limit is 50 MB for documents; 50 MiB matches that
/// and stays an order of magnitude below typical operator disk
/// budgets, while comfortably covering photo + document payloads
/// teamctl operators actually exchange. Bump requires a release-notes
/// entry + this constant — #332.
const MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;

/// Operator-facing error when Telegram already reports a media file
/// over the ceiling — fast-fail before opening any sink. The `kind`
/// label distinguishes voice (RAM-OOM defense, #279) from media
/// (disk-fill defense, #332) so operators can tell which boundary
/// fired without log-diving. Pure for unit-testability.
fn media_size_pre_reject(kind: &str, reported: u32, max: usize) -> String {
    format!("{kind} too large: {reported} bytes (max {max})")
}

/// Operator-facing error when cumulative bytes mid-download would pass
/// the ceiling — fires from [`BoundedWriter`] when `file.size` lied
/// small but the actual stream tries to flood us. The `kind` label
/// threads from the wrapper's construction so the message names the
/// right boundary (`voice file` vs `media file`).
fn media_size_mid_reject(kind: &str, sofar: usize, incoming: usize, max: usize) -> String {
    let proposed = sofar.saturating_add(incoming);
    format!(
        "{kind} exceeded ceiling mid-download \
         (max {max} bytes; upstream sent at least {proposed})"
    )
}

// #279 voice-path pre-check helper — thin wrapper over the generic
// pair above so the voice pre-reject call site stays readable. No
// `voice_size_mid_reject` wrapper needed: `BoundedWriter::poll_write`
// calls `media_size_mid_reject` directly with the kind from the
// wrapper's construction.
fn voice_size_pre_reject(reported: u32, max: usize) -> String {
    media_size_pre_reject("voice file", reported, max)
}

/// `tokio::io::AsyncWrite` adapter that aborts with
/// `io::ErrorKind::InvalidData` once cumulative bytes would exceed
/// `max`. Wraps the voice-download `Vec<u8>` (#279) AND the disk-write
/// `tokio::fs::File` handle in [`download_to`] (#332) so a lying
/// upstream can't stream past the ceiling even when Telegram's
/// reported `file.size` looked small. Generic over `W: AsyncWrite +
/// Unpin + Send`, so the same wrapper works at both sinks; the `kind`
/// label threads into mid-download error messages so operators can
/// tell which boundary fired.
struct BoundedWriter<W> {
    kind: &'static str,
    inner: W,
    max: usize,
    written: usize,
}

impl<W> BoundedWriter<W> {
    fn new(kind: &'static str, inner: W, max: usize) -> Self {
        Self {
            kind,
            inner,
            max,
            written: 0,
        }
    }
    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for BoundedWriter<W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.written.saturating_add(buf.len()) > this.max {
            let msg = media_size_mid_reject(this.kind, this.written, buf.len(), this.max);
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                msg,
            )));
        }
        match std::pin::Pin::new(&mut this.inner).poll_write(cx, buf) {
            std::task::Poll::Ready(Ok(n)) => {
                this.written = this.written.saturating_add(n);
                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// Stream a Telegram-hosted voice file into memory. Audio is ~tens of
/// KB per voice note (Telegram caps voice at 60s OGG OPUS), so we
/// collect into a `Vec<u8>` rather than touching disk — the bytes hit
/// the Groq multipart body and are dropped, no on-disk artifact
/// survives.
///
/// #279: both the pre-allocation and the actual download are bounded
/// by [`MAX_VOICE_BYTES`]. The pre-check on `file.size` fast-fails when
/// Telegram already says it's too large; the [`BoundedWriter`] wrapping
/// the destination `Vec<u8>` catches a lying-small upstream that tries
/// to stream past the ceiling mid-download. Either rejection bubbles
/// up as a clear, operator-visible error naming both numbers.
async fn download_voice_bytes(bot: &Bot, file_id: &str) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    let file = bot
        .get_file(file_id)
        .await
        .map_err(|e| format!("get_file: {e}"))?;

    if (file.size as usize) > MAX_VOICE_BYTES {
        return Err(voice_size_pre_reject(file.size, MAX_VOICE_BYTES));
    }

    let buf: Vec<u8> = Vec::with_capacity(file.size as usize);
    let mut bounded = BoundedWriter::new("voice file", buf, MAX_VOICE_BYTES);
    bot.download_file(&file.path, &mut bounded)
        .await
        .map_err(|e| format!("download_file: {e}"))?;

    let mut buf = bounded.into_inner();
    buf.flush().await.ok();
    Ok(buf)
}

/// Provider dispatch. v1 has one arm (`groq`); adding OpenAI Whisper or
/// whisper.cpp later is one match arm here, no plugin framework needed.
async fn transcribe(audio: &[u8], stt: &SttRuntime) -> SttOutcome {
    match stt.provider.as_str() {
        "groq" => transcribe_groq(audio, stt).await,
        other => SttOutcome::Failed(format!("unknown stt provider `{other}`")),
    }
}

/// Groq Whisper transcription. The OpenAI-compatible endpoint accepts a
/// multipart form with `file`, `model`, optional `language`, and
/// `response_format=text` for a plain-text body. An empty/whitespace
/// response is treated as `Skipped` (silence, music, noise) — non-empty
/// transcripts are `Ok`.
async fn transcribe_groq(audio: &[u8], stt: &SttRuntime) -> SttOutcome {
    let part = match reqwest::multipart::Part::bytes(audio.to_vec())
        .file_name("voice.ogg")
        .mime_str("audio/ogg")
    {
        Ok(p) => p,
        Err(e) => return SttOutcome::Failed(format!("multipart: {e}")),
    };
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", stt.model.clone())
        .text("response_format", "text");
    if let Some(lang) = &stt.language {
        form = form.text("language", lang.clone());
    }

    let resp = stt
        .http
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(&stt.api_key)
        .multipart(form)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return SttOutcome::Failed(format!("groq request: {e}")),
    };
    let status = resp.status();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => return SttOutcome::Failed(format!("groq read body: {e}")),
    };
    if !status.is_success() {
        return SttOutcome::Failed(format!("groq {status}: {}", body.trim()));
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        SttOutcome::Skipped
    } else {
        SttOutcome::Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// T-104: `/readnow ` is the case-sensitive, single-space-separated
    /// prefix that flips a Telegram-routed message to immediate
    /// (full-body inline) delivery. Anything else is a regular lazy row.
    #[test]
    fn peel_readnow_strips_prefix_when_present() {
        assert_eq!(
            peel_readnow("/readnow build broke"),
            ("build broke", Some("immediate")),
        );
    }

    #[test]
    fn peel_readnow_passes_through_when_prefix_absent() {
        assert_eq!(peel_readnow("regular message"), ("regular message", None));
    }

    #[test]
    fn peel_readnow_is_case_sensitive() {
        // `/ReadNow` and `/READNOW` are not the literal prefix — pass
        // through. Avoids surprising lowercase-vs-mixed-case false hits.
        assert_eq!(peel_readnow("/ReadNow x"), ("/ReadNow x", None));
        assert_eq!(peel_readnow("/READNOW x"), ("/READNOW x", None));
    }

    #[test]
    fn peel_readnow_requires_single_space_separator() {
        // `/readnowfoo` is not the prefix; `/readnow  x` (double space)
        // strips only the first space and the second space stays in body.
        assert_eq!(peel_readnow("/readnowfoo"), ("/readnowfoo", None));
        assert_eq!(peel_readnow("/readnow  x"), (" x", Some("immediate")));
    }

    #[test]
    fn peel_readnow_with_empty_body_after_prefix() {
        // Operator typed only `/readnow ` — preserve the empty body so
        // the caller can reject (sending an empty immediate row would be
        // useless). Caller is responsible for the empty-body guard.
        assert_eq!(peel_readnow("/readnow "), ("", Some("immediate")));
    }

    #[test]
    fn approval_outcome_line_uses_approver_first_name() {
        assert_eq!(approval_outcome_line(true, "Hamed"), "✅ Approved by Hamed",);
        assert_eq!(
            approval_outcome_line(false, "Hamed"),
            "❌ Rejected by Hamed",
        );
    }

    #[test]
    fn approval_outcome_line_handles_unicode_first_name() {
        assert_eq!(
            approval_outcome_line(true, "علیرضا"),
            "✅ Approved by علیرضا",
        );
    }

    // ── #299 multi-option decision helpers ───────────────────────

    #[test]
    fn decision_and_cancel_outcome_lines_name_the_chooser() {
        assert_eq!(
            decision_outcome_line("Ship it", "Hamed"),
            "✅ Ship it — chosen by Hamed",
        );
        assert_eq!(cancel_outcome_line("Hamed"), "🚫 Cancelled by Hamed");
        // Unicode chooser parity with the binary outcome-line test.
        assert_eq!(
            decision_outcome_line("گزینه", "علیرضا"),
            "✅ گزینه — chosen by علیرضا",
        );
    }

    #[test]
    fn parse_callback_accepts_all_four_verbs() {
        assert_eq!(parse_callback("approve:7"), Some((7, CbAction::Approve)));
        assert_eq!(parse_callback("deny:7"), Some((7, CbAction::Deny)));
        assert_eq!(parse_callback("cancel:42"), Some((42, CbAction::Cancel)));
        assert_eq!(parse_callback("opt:42:2"), Some((42, CbAction::Opt(2))));
    }

    #[test]
    fn parse_callback_rejects_malformed() {
        // No colon, unparseable id, unknown verb, bad opt idx, and
        // trailing junk all return None — handle_callback then ignores
        // the tap, preserving the prior unknown-data behavior.
        assert_eq!(parse_callback("approve"), None);
        assert_eq!(parse_callback("approve:x"), None);
        assert_eq!(parse_callback("frobnicate:7"), None);
        assert_eq!(parse_callback("opt:7:notanum"), None);
        assert_eq!(parse_callback("opt:7"), None);
        assert_eq!(parse_callback("approve:7:8"), None);
        assert_eq!(parse_callback("cancel:7:8"), None);
    }

    #[test]
    fn decode_options_handles_null_and_garbage() {
        assert!(decode_options(None).is_empty());
        assert!(decode_options(Some("not json")).is_empty());
        // Entries missing label or value are filtered, not fatal.
        assert_eq!(
            decode_options(Some(r#"[{"label":"A","value":"a"},{"value":"b"}]"#)),
            vec![("A".to_string(), "a".to_string())],
        );
        assert_eq!(
            decode_options(Some(
                r#"[{"label":"Yes","value":"y"},{"label":"No","value":"n"}]"#
            )),
            vec![
                ("Yes".to_string(), "y".to_string()),
                ("No".to_string(), "n".to_string()),
            ],
        );
    }

    /// Pull `(text, callback_data)` out of a keyboard for assertions.
    fn kb_pairs(kb: &InlineKeyboardMarkup) -> Vec<(String, String)> {
        use teloxide::types::InlineKeyboardButtonKind::CallbackData;
        kb.inline_keyboard
            .iter()
            .flatten()
            .map(|b| {
                let data = match &b.kind {
                    CallbackData(d) => d.clone(),
                    _ => panic!("expected callback button"),
                };
                (b.text.clone(), data)
            })
            .collect()
    }

    #[test]
    fn approval_keyboard_empty_options_is_binary_back_compat() {
        // The binary card must stay byte-identical to the pre-#299
        // render: exactly Approve/Deny with `approve:{id}`/`deny:{id}`.
        let kb = approval_keyboard(7, &[]);
        assert_eq!(
            kb_pairs(&kb),
            vec![
                ("Approve".to_string(), "approve:7".to_string()),
                ("Deny".to_string(), "deny:7".to_string()),
            ],
        );
        assert_eq!(kb.inline_keyboard.len(), 1, "binary is a single row");
    }

    #[test]
    fn approval_keyboard_multi_renders_options_then_cancel() {
        let opts = vec![
            ("Ship".to_string(), "ship".to_string()),
            ("Hold".to_string(), "hold".to_string()),
            ("Rework".to_string(), "rework".to_string()),
        ];
        let kb = approval_keyboard(9, &opts);
        assert_eq!(
            kb_pairs(&kb),
            vec![
                ("Ship".to_string(), "opt:9:0".to_string()),
                ("Hold".to_string(), "opt:9:1".to_string()),
                ("Rework".to_string(), "opt:9:2".to_string()),
                ("Cancel".to_string(), "cancel:9".to_string()),
            ],
        );
        assert_eq!(
            kb.inline_keyboard.len(),
            4,
            "one row per option + a Cancel row"
        );
    }

    fn seed(conn: &Connection) {
        team_core::mailbox::ensure(conn).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name) VALUES ('p','P')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
             VALUES ('p:eng_lead','p','eng_lead','claude-code',1,NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
             VALUES ('p:dev1','p','dev1','claude-code',0,'eng_lead')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
             VALUES ('p:pm','p','pm','claude-code',1,NULL)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn manager_of_returns_self_for_a_manager() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        assert_eq!(
            manager_of(&conn, "p:eng_lead").as_deref(),
            Some("p:eng_lead")
        );
        assert_eq!(manager_of(&conn, "p:pm").as_deref(), Some("p:pm"));
    }

    #[test]
    fn manager_of_resolves_reports_to_for_a_worker() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        assert_eq!(manager_of(&conn, "p:dev1").as_deref(), Some("p:eng_lead"));
    }

    #[test]
    fn manager_of_returns_none_for_unknown_agent() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        assert!(manager_of(&conn, "p:ghost").is_none());
    }

    // ── T-086-A dispatch tests ──────────────────────────────────

    #[test]
    fn classify_kind_treats_null_and_empty_as_text() {
        // Back-compat pin: rows from before T-086-A migration have NULL
        // kind; rows inserted via legacy `send_dm` still leave it NULL.
        // Both must dispatch as plain text — otherwise older databases
        // would suddenly fail the unknown-kind path.
        assert_eq!(classify_kind(None), DispatchKind::Text);
        assert_eq!(classify_kind(Some("text")), DispatchKind::Text);
        assert_eq!(classify_kind(Some("")), DispatchKind::Text);
    }

    #[test]
    fn classify_kind_routes_image_and_file() {
        assert_eq!(classify_kind(Some("image")), DispatchKind::Image);
        assert_eq!(classify_kind(Some("file")), DispatchKind::File);
    }

    #[test]
    fn classify_kind_falls_back_for_unknown_kinds() {
        // Forward-compat: kinds the binary doesn't recognise surface as
        // a text fallback rather than panicking. T-086-A's prophetic
        // example ("reaction") landed in T-086-E and now routes to its
        // own arm (covered by `classify_kind_routes_reaction`); the
        // fallback test stays useful by pinning truly-unknown strings.
        assert_eq!(
            classify_kind(Some("garbage")),
            DispatchKind::UnknownFallback
        );
        assert_eq!(classify_kind(Some("custom")), DispatchKind::UnknownFallback);
    }

    #[test]
    fn parse_payload_extracts_source_value_and_caption() {
        let p = parse_payload(r#"{"source":"path","value":"/tmp/x.png","caption":"hi"}"#)
            .expect("payload parses");
        assert_eq!(p.source, "path");
        assert_eq!(p.value, "/tmp/x.png");
        assert_eq!(p.caption.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_payload_handles_missing_caption() {
        let p = parse_payload(r#"{"source":"url","value":"https://x.test/a.png"}"#)
            .expect("payload parses");
        assert_eq!(p.source, "url");
        assert!(p.caption.is_none());
    }

    #[test]
    fn parse_payload_returns_none_on_garbage() {
        assert!(parse_payload("not json").is_none());
        assert!(
            parse_payload(r#"{"value":"x"}"#).is_none(),
            "missing source"
        );
        assert!(
            parse_payload(r#"{"source":"path"}"#).is_none(),
            "missing value"
        );
    }

    #[test]
    fn input_file_from_path_and_url_both_construct() {
        // We can't easily assert teloxide internals, but we can pin that
        // both branches return Some() — the negative case (unknown
        // source) is the regression risk and is covered by the next
        // test.
        let p = parse_payload(r#"{"source":"path","value":"/tmp/x.png"}"#).unwrap();
        assert!(input_file_from(&p).is_some());
        let p = parse_payload(r#"{"source":"url","value":"https://x.test/a.png"}"#).unwrap();
        assert!(input_file_from(&p).is_some());
    }

    #[test]
    fn input_file_from_unknown_source_returns_none() {
        let p = MediaPayload {
            source: "bytes".into(),
            value: "abc".into(),
            caption: None,
        };
        assert!(input_file_from(&p).is_none());
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_row(
        conn: &Connection,
        sender: &str,
        text: &str,
        kind: Option<&str>,
        payload: Option<&str>,
        telegram_msg_id: Option<i64>,
    ) -> i64 {
        let project = sender.split_once(':').map(|(p, _)| p).unwrap_or("p");
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at,
                 kind, structured_payload, telegram_msg_id)
             VALUES (?1, ?2, 'user:telegram', ?3, strftime('%s','now'), ?4, ?5, ?6)",
            params![project, sender, text, kind, payload, telegram_msg_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// SELECT shape `outbound_loop` runs — kept in sync with the production
    /// query so MailboxRow's column ordering stays asserted.
    const OUTBOUND_SELECT: &str =
        "SELECT m.id, m.sender, m.text, m.kind, m.structured_payload, m.telegram_msg_id
         FROM messages m
         WHERE m.id > ?1
           AND m.recipient = 'user:telegram'
           AND m.acked_at IS NULL
         ORDER BY m.id";

    #[test]
    fn outbound_select_returns_kind_and_payload_for_structured_rows() {
        // Pins the SELECT-shape contract: outbound_loop's enriched query
        // surfaces both new columns so the dispatcher can route on them.
        // Without this, a structured row would still be fetched but with
        // text-row defaults — silently degrading image/file to text.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_row(
            &conn,
            "p:eng_lead",
            "shot",
            Some("image"),
            Some(r#"{"source":"path","value":"/tmp/a.png"}"#),
            None,
        );
        let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
        let rows: Vec<MailboxRow> = stmt
            .query_map(params![0i64], MailboxRow::from_row)
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].kind.as_deref(), Some("image"));
        assert!(rows[0].payload.as_deref().unwrap().contains("/tmp/a.png"));
    }

    #[test]
    fn outbound_select_returns_null_kind_for_legacy_text_rows() {
        // Pre-T-086-A rows (and rows written by `send_dm`, which leaves
        // kind NULL) still surface in the SELECT — the dispatcher's
        // classify_kind treats NULL as Text, completing the back-compat
        // round-trip.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_row(&conn, "p:eng_lead", "hello", None, None, None);
        let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
        let rows: Vec<MailboxRow> = stmt
            .query_map(params![0i64], MailboxRow::from_row)
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert!(rows[0].kind.is_none());
        assert!(rows[0].payload.is_none());
        assert!(rows[0].telegram_msg_id.is_none());
        assert_eq!(classify_kind(rows[0].kind.as_deref()), DispatchKind::Text);
    }

    #[test]
    fn outbound_select_returns_telegram_msg_id_when_set_for_threaded_rows() {
        // T-086-B: outbound rows written with a `reply_to_message_id` carry
        // it forward via `telegram_msg_id`. The dispatcher reads this and
        // attaches `reply_parameters` on send. Pinning the round-trip
        // guards against future SELECT shape regressions silently dropping
        // the threading column.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_row(&conn, "p:eng_lead", "ack", None, None, Some(7777));
        let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
        let rows: Vec<MailboxRow> = stmt
            .query_map(params![0i64], MailboxRow::from_row)
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].telegram_msg_id, Some(7777));
    }

    #[test]
    fn render_html_paired_bold_emphasis() {
        assert_eq!(render_html("**bold** text"), "<b>bold</b> text");
        assert_eq!(render_html("__also bold__"), "<b>also bold</b>");
    }

    #[test]
    fn render_html_paired_italic_and_inline_code() {
        assert_eq!(render_html("*italic* text"), "<i>italic</i> text");
        assert_eq!(
            render_html("plain `code` here"),
            "plain <code>code</code> here"
        );
    }

    #[test]
    fn render_html_translates_list_bullets() {
        let input = "- one\n- two\n  * nested\n+ three";
        let expected = "• one\n• two\n  • nested\n• three";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_preserves_emoji_and_converts_inline() {
        let input = "🔐 deploy\nrouting prompt to one channel — the **right** one";
        let expected = "🔐 deploy\nrouting prompt to one channel — the <b>right</b> one";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_leaves_single_underscore_alone() {
        // Underscore is too common in dev text to convert. `thread_id`,
        // `snake_case_var`, `path/with_underscore` must all survive.
        assert_eq!(render_html("thread_id"), "thread_id");
        assert_eq!(render_html("snake_case_var here"), "snake_case_var here");
        assert_eq!(render_html("_underscored_"), "_underscored_");
    }

    #[test]
    fn render_html_unmatched_delimiters_pass_through() {
        // Single `*` or `` ` `` with no closing partner on the same line
        // must reach the operator verbatim. The regression that motivated
        // T-134 was `array[i] = b * 2` losing its `*`.
        assert_eq!(render_html("array[i] = b * 2"), "array[i] = b * 2");
        assert_eq!(render_html("unmatched `tick"), "unmatched `tick");
        assert_eq!(
            render_html("unmatched **bold-open"),
            "unmatched **bold-open"
        );
    }

    #[test]
    fn render_html_pairing_is_per_line() {
        // An open `*` on one line cannot pair with a `*` on the next.
        let input = "*open\nclose*";
        assert_eq!(render_html(input), "*open\nclose*");
    }

    #[test]
    fn render_html_escapes_lt_gt_amp_in_raw_text() {
        // Quoting a `<channel>` tag must not break Telegram's HTML
        // parser AND must not drop characters.
        assert_eq!(
            render_html("<channel source=\"team\"> & friends"),
            "&lt;channel source=\"team\"&gt; &amp; friends",
        );
    }

    #[test]
    fn render_html_escapes_inside_inline_code() {
        // Telegram's HTML parser requires escaping inside `<code>` too.
        assert_eq!(
            render_html("see `<thing>` for more"),
            "see <code>&lt;thing&gt;</code> for more",
        );
    }

    #[test]
    fn render_html_fenced_block_no_language() {
        let input = "before\n```\nlet x = 1;\n```\nafter";
        let expected = "before\n<pre>let x = 1;</pre>\nafter";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_fenced_block_with_language_tag() {
        let input = "```rust\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_fenced_block_escapes_html_inside() {
        let input = "```\n<channel> & co\n```";
        let expected = "<pre>&lt;channel&gt; &amp; co</pre>";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_unmatched_fence_falls_through_as_normal_line() {
        // A lone ``` with no closer must not swallow the rest of the
        // message — emit it as a regular line (escaping handles the
        // backticks fine, since they're delimiters not html-special).
        let input = "```\nstray";
        // Backticks-only opener falls through to inline pass; the lone
        // ``` becomes a `<code>` opener with no close, which itself
        // falls through → literal. Result is the input verbatim.
        assert_eq!(render_html(input), "```\nstray");
    }

    #[test]
    fn html_escape_str_escapes_the_three_html_specials_only() {
        // Pin the exact escape table — T-140 leans on this for the HITL
        // approval card (`agent`/`action`), the `forward_row`
        // attribution suffix (`row.sender`), and the fence-marker
        // language tag. Quote chars are intentionally NOT escaped
        // because they only matter inside attributes, and the only
        // attribute we emit (`class="language-…"`) substitutes a tag
        // that's already been through this escape.
        assert_eq!(
            html_escape_str("<channel> & friends"),
            "&lt;channel&gt; &amp; friends",
        );
        assert_eq!(
            html_escape_str("safe-text_with.no.specials"),
            "safe-text_with.no.specials",
        );
        // Quotes pass through verbatim — DiD relies on this NOT being
        // escaped so the renderer doesn't double-encode operator-typed
        // quotes inside otherwise-plain agent text.
        assert_eq!(html_escape_str("she said \"hi\""), "she said \"hi\"");
    }

    #[test]
    fn fence_marker_takes_leading_alphanumeric_dash_underscore_run_as_lang() {
        // T-149: the language tag is the leading run of `[A-Za-z0-9_-]`
        // — any other byte (whitespace, quote, slash, non-ASCII)
        // ends the tag. Cases inherited from T-140 (whitespace
        // termination) still hold under the tighter rule.
        assert_eq!(fence_marker("```").as_deref(), Some(""));
        assert_eq!(fence_marker("```rust").as_deref(), Some("rust"));
        assert_eq!(fence_marker("```rust // example").as_deref(), Some("rust"));
        assert_eq!(
            fence_marker("    ```python   extra junk").as_deref(),
            Some("python"),
        );
        assert_eq!(fence_marker("not a fence").as_deref(), None);
    }

    #[test]
    fn fence_marker_admits_dash_and_underscore_in_lang() {
        // Real-world language tags use both — `shell-script`,
        // `objective-c`, `objective_c` all need to round-trip so we
        // don't regress real syntax-highlighter classes.
        assert_eq!(
            fence_marker("```shell-script").as_deref(),
            Some("shell-script"),
        );
        assert_eq!(
            fence_marker("```objective-c").as_deref(),
            Some("objective-c"),
        );
        assert_eq!(
            fence_marker("```objective_c").as_deref(),
            Some("objective_c"),
        );
        // Leading dash/underscore is intentionally allowed: the
        // `take_while` predicate has no leading-char restriction,
        // and both chars are in the allowed set so there's zero
        // injection risk. Pin the shape so a future reader doesn't
        // wonder whether `-rust` should have been rejected (per
        // ada's #154 peer review).
        assert_eq!(fence_marker("```-rust").as_deref(), Some("-rust"));
        assert_eq!(fence_marker("```_rust").as_deref(), Some("_rust"));
    }

    #[test]
    fn fence_marker_truncates_lang_at_quote_for_attribute_injection_safety() {
        // T-149 regression: `html_escape_into` escapes `<>&` but NOT
        // `"`. A fence like ```` ```"x ```` previously yielded
        // `lang = "\"x"` which then landed inside `class="language-…"`
        // and broke the parser. The tighter parse-boundary rule
        // ends the tag at the first non-`[A-Za-z0-9_-]` byte, so the
        // quote (and anything after it) is dropped.
        assert_eq!(fence_marker("```\"x").as_deref(), Some(""));
        assert_eq!(fence_marker("```rust\"injected\"").as_deref(), Some("rust"));
        assert_eq!(fence_marker("```\"").as_deref(), Some(""));
    }

    #[test]
    fn fence_marker_truncates_lang_at_other_punctuation() {
        // Slashes, dots, and other punctuation also end the tag —
        // none of these belong in a syntax-highlighter class anyway,
        // and dropping them is the safer default than letting them
        // through into the rendered HTML.
        assert_eq!(fence_marker("```ru/st").as_deref(), Some("ru"));
        assert_eq!(fence_marker("```py.thon").as_deref(), Some("py"));
        assert_eq!(fence_marker("```rust!").as_deref(), Some("rust"));
        assert_eq!(fence_marker("```c++").as_deref(), Some("c"));
    }

    #[test]
    fn fence_marker_truncates_lang_at_non_ascii() {
        // Non-ASCII chars (emoji, unicode letters) are valid Rust
        // identifier characters under `is_alphanumeric()` but
        // shouldn't appear in a syntax-highlighter class. Restrict
        // to ASCII alphanumerics so an agent emitting ```` ```rüst ````
        // produces an empty lang tag (renders as plain `<pre>`)
        // instead of a class attribute with non-ASCII bytes.
        assert_eq!(fence_marker("```rüst").as_deref(), Some("r"));
        assert_eq!(fence_marker("```🦀rust").as_deref(), Some(""));
        assert_eq!(fence_marker("```rust🦀").as_deref(), Some("rust"));
    }

    #[test]
    fn render_html_fenced_block_drops_injected_quote_in_lang_tag() {
        // T-149 round-trip: an agent emitting a fence with a quote
        // in the lang tag gets a clean fallback (empty tag → plain
        // `<pre>`) — the prior parser would have produced
        // `<pre><code class="language-"x">…` and broken Telegram's
        // HTML parser.
        let input = "```\"x\nbody\n```";
        let expected = "<pre>body</pre>";
        assert_eq!(render_html(input), expected);

        // Quote-after-valid-lang case: `rust` survives, the quote
        // and everything after it is dropped before the class
        // attribute is built.
        let input = "```rust\"injected\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn hitl_card_text_format_pins_agent_then_action_then_summary() {
        // Pin the HITL approval card's format-string composition so
        // a future edit can't silently swap `agent`/`action` order or
        // drop one of the escapes — the standalone
        // `html_escape_str_escapes_the_three_html_specials_only` test
        // covers the function, not the composition (per ada's #145
        // peer review).
        let id: i64 = 42;
        let agent = "pm";
        let action = "approve";
        let summary = "ship the **release**";
        let actual = format!(
            "🔐 #{id}  {}\naction: {}\n{}",
            html_escape_str(agent),
            html_escape_str(action),
            render_html(summary),
        );
        assert_eq!(
            actual,
            "🔐 #42  pm\naction: approve\nship the <b>release</b>",
        );

        // And the in-schema-but-with-html-specials variant — confirms
        // escape fires before the format-string lands on Telegram.
        let actual_escaped = format!(
            "🔐 #{id}  {}\naction: {}\n{}",
            html_escape_str("ops:<bot>"),
            html_escape_str("kill & restart"),
            render_html(summary),
        );
        assert_eq!(
            actual_escaped,
            "🔐 #42  ops:&lt;bot&gt;\naction: kill &amp; restart\nship the <b>release</b>",
        );
    }

    #[test]
    fn render_html_fenced_block_strips_trailing_lang_garbage() {
        // T-140 round-trip: a fence with `lang + comment` reaches the
        // operator with a clean `class="language-lang"` attribute.
        let input = "```rust // example\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(render_html(input), expected);
    }

    #[test]
    fn render_html_inline_code_is_not_re_parsed() {
        // Backtick content is NOT recursively converted — `**bold**`
        // inside a code span renders as literal text, not as <b>.
        assert_eq!(render_html("`**not bold**`"), "<code>**not bold**</code>",);
    }

    /// T-036 — exercise the SQL ordering pattern used by `handle_callback`
    /// (and by `cmd::approval::decide` in teamctl) directly against a
    /// `Connection` so the ordering invariant has a unit-testable home.
    /// Asserts: a stale tap on an `undeliverable` row does *not* flip
    /// `delivered_at` (preserving the invariant
    /// `undeliverable ↔ delivered_at IS NULL`), and a live tap on a
    /// `pending` row flips both fields atomically.
    fn decide_sql(conn: &Connection, id: i64, approved: bool) -> bool {
        let status = if approved { "approved" } else { "denied" };
        let n = conn
            .execute(
                "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'), decided_by='user:telegram'
                 WHERE id=?2 AND status='pending'",
                params![status, id],
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if n {
            let _ = conn.execute(
                "UPDATE approvals SET delivered_at=strftime('%s','now')
                 WHERE id=?1 AND delivered_at IS NULL",
                params![id],
            );
        }
        n
    }

    fn insert_approval(conn: &Connection, status: &str, delivered_at: Option<f64>) -> i64 {
        conn.execute(
            "INSERT INTO approvals (project_id, agent_id, action, summary, status,
                                    requested_at, expires_at, delivered_at)
             VALUES ('p', 'eng_lead', 'publish', 's', ?1, 0.0, 999999999.0, ?2)",
            params![status, delivered_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn stale_tap_on_undeliverable_does_not_flip_delivered_at() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_approval(&conn, "undeliverable", None);

        let decided = decide_sql(&conn, id, true);
        assert!(!decided, "stale tap should report no live decision");

        let (status, delivered_at): (String, Option<f64>) = conn
            .query_row(
                "SELECT status, delivered_at FROM approvals WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "undeliverable");
        assert!(
            delivered_at.is_none(),
            "delivered_at must stay NULL on undeliverable row (invariant)"
        );
    }

    #[test]
    fn live_tap_on_pending_flips_status_and_delivered_at() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_approval(&conn, "pending", None);

        let decided = decide_sql(&conn, id, true);
        assert!(decided, "live tap should report decision");

        let (status, delivered_at): (String, Option<f64>) = conn
            .query_row(
                "SELECT status, delivered_at FROM approvals WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "approved");
        assert!(
            delivered_at.is_some(),
            "live decision implies delivery acknowledgement"
        );
    }

    /// T-039 — unscoped bot's back-compat path: when `state.manager` is
    /// `None`, every approval routes to this bot regardless of which
    /// agent filed it. The fallback is what makes pre-T-027 setups
    /// (single team-wide bot) keep working after per-manager scoping
    /// landed.
    #[test]
    fn unscoped_bot_routes_every_approval() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        // Worker, manager, and an unknown id all route through.
        assert!(should_route(None, "p:dev1", &conn));
        assert!(should_route(None, "p:eng_lead", &conn));
        assert!(should_route(None, "p:ghost", &conn));
        // Even agents from a different (unseeded) project route through —
        // the unscoped bot is intentionally undiscriminating.
        assert!(should_route(None, "other:agent", &conn));
    }

    #[test]
    fn scoped_bot_routes_only_its_managers_chain() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        // Bot scoped to p:eng_lead. dev1 reports to eng_lead → routes.
        assert!(should_route(Some("p:eng_lead"), "p:dev1", &conn));
        // The manager themselves routes (manager_of returns self).
        assert!(should_route(Some("p:eng_lead"), "p:eng_lead", &conn));
        // pm is a sibling manager — does NOT route to eng_lead's bot.
        assert!(!should_route(Some("p:eng_lead"), "p:pm", &conn));
    }

    #[test]
    fn scoped_bot_with_unknown_agent_falls_back_to_self_routing() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        // Unknown agent: manager_of returns None → routed = agent_id;
        // routed != scoped → does not route. This pins the fallback rule
        // (don't surface unknown rows to a scoped bot) so a future
        // change can't silently relax it.
        assert!(!should_route(Some("p:eng_lead"), "p:ghost", &conn));
    }

    fn insert_reply(conn: &Connection, sender: &str, text: &str) -> i64 {
        let project = sender.split_once(':').map(|(p, _)| p).unwrap_or("p");
        conn.execute(
            "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
             VALUES (?1, ?2, 'user:telegram', ?3, strftime('%s','now'))",
            params![project, sender, text],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Regression: when two managers (`p:pm`, `p:eng_lead`) live in the same
    /// project and each has its own scoped bot, a `reply_to_user` from one
    /// manager must surface in *that* manager's bot only — not in sibling
    /// bots. Pre-fix the project-id SQL filter was the only filter so all
    /// in-project bots fanned out the same reply.
    #[test]
    fn reply_routes_only_to_its_senders_bot() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let pm_msg = insert_reply(&conn, "p:pm", "from pm");
        let eng_msg = insert_reply(&conn, "p:eng_lead", "from eng");

        // Pull the project-scoped pre-filter rows the way outbound_loop does.
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.sender, m.text FROM messages m
                 WHERE m.id > 0
                   AND m.recipient = 'user:telegram'
                   AND m.acked_at IS NULL
                   AND m.project_id = 'p'
                 ORDER BY m.id",
            )
            .unwrap();
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(rows.len(), 2, "both replies share the project pre-filter");

        // pm bot keeps only the pm reply.
        let pm_routed: Vec<i64> = rows
            .iter()
            .filter(|(_, sender, _)| should_route(Some("p:pm"), sender, &conn))
            .map(|(id, _, _)| *id)
            .collect();
        assert_eq!(pm_routed, vec![pm_msg]);

        // eng_lead bot keeps only the eng_lead reply.
        let eng_routed: Vec<i64> = rows
            .iter()
            .filter(|(_, sender, _)| should_route(Some("p:eng_lead"), sender, &conn))
            .map(|(id, _, _)| *id)
            .collect();
        assert_eq!(eng_routed, vec![eng_msg]);

        // Unscoped bot back-compat: forwards both.
        let unscoped: Vec<i64> = rows
            .iter()
            .filter(|(_, sender, _)| should_route(None, sender, &conn))
            .map(|(id, _, _)| *id)
            .collect();
        assert_eq!(unscoped, vec![pm_msg, eng_msg]);
    }

    #[test]
    fn live_tap_keeps_existing_delivered_at_unchanged() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let id = insert_approval(&conn, "pending", Some(1234.5));

        let decided = decide_sql(&conn, id, false);
        assert!(decided);

        let delivered_at: f64 = conn
            .query_row(
                "SELECT delivered_at FROM approvals WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            (delivered_at - 1234.5).abs() < 1e-6,
            "previously-set delivered_at must not be overwritten ({delivered_at})"
        );
    }

    // ── T-086-G slash-passthrough ─────────────────────────────────

    #[test]
    fn agent_runtime_returns_runtime_for_known_agent() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        // `seed` inserts p:eng_lead (manager, runtime "claude-code"),
        // p:pm (manager), and p:dev1 (worker).
        assert_eq!(
            agent_runtime(&conn, "p:eng_lead"),
            Some("claude-code".into())
        );
    }

    #[test]
    fn agent_runtime_returns_runtime_when_runtime_varies() {
        // Hand-extend the seed with a non-CC manager so the lookup
        // path is exercised against a runtime that the slash-passthrough
        // gate would later reject.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
             VALUES ('p:codex_mgr','p','codex_mgr','codex',1,NULL)",
            [],
        )
        .unwrap();
        assert_eq!(agent_runtime(&conn, "p:codex_mgr"), Some("codex".into()));
    }

    #[test]
    fn agent_runtime_returns_none_for_unknown_agent() {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        assert_eq!(agent_runtime(&conn, "p:ghost"), None);
    }

    // ── T-367: first-connect greeting + /help split ──────────────────

    #[test]
    fn start_greeting_is_one_liner_using_manager_id_fallback() {
        // /start on a scoped bot without a display_name: short one-liner
        // naming the manager id, and crucially NO command dump.
        let body = start_help_body(false, Some("software-team:director"));
        assert_eq!(
            body,
            "Connected to software-team:director via teamctl. Just type to chat."
        );
        assert!(
            !body.contains("/pending"),
            "greeting must not dump commands"
        );
        assert!(!body.contains("/dm"), "greeting must not dump commands");
    }

    #[test]
    fn start_greeting_prefers_display_name() {
        // /start prefers the friendly display_name (T-160) when set.
        let body = start_help_body(false, Some("Director"));
        assert_eq!(
            body,
            "Connected to Director via teamctl. Just type to chat."
        );
    }

    #[test]
    fn start_greeting_unscoped_points_to_help() {
        // Unscoped bot has no single manager to name — still a one-liner,
        // and it points the operator at /help for the command list.
        let body = start_help_body(false, None);
        assert_eq!(body, "Connected via teamctl. Send /help for commands.");
        assert!(!body.contains("/dm"), "greeting must not dump commands");
    }

    #[test]
    fn help_lists_advanced_commands_for_scoped_bot() {
        // /help is where the power-user commands live now.
        let body = start_help_body(true, Some("Director"));
        assert!(body.contains("/pending"), "help must list /pending");
        assert!(body.contains("/dm"), "help must list /dm");
        assert!(
            body.contains("slash-passthrough to Director's tmux session (Claude Code only)"),
            "help must list slash-passthrough naming the manager: {body}"
        );
        assert!(
            body.contains("chat with Director"),
            "help should still tell the operator how to chat: {body}"
        );
    }

    #[test]
    fn help_lists_dm_and_pending_for_unscoped_bot() {
        // Unscoped /help keeps the dm + pending commands discoverable;
        // slash-passthrough is manager-scoped only, so it's absent here.
        let body = start_help_body(true, None);
        assert!(
            body.contains("/dm <project>:<agent>"),
            "unscoped help must list /dm"
        );
        assert!(
            body.contains("/pending"),
            "unscoped help must list /pending"
        );
        assert!(
            !body.contains("slash-passthrough"),
            "unscoped bot has no manager tmux session to pass through to: {body}"
        );
    }

    #[test]
    fn slash_outcome_passes_through_for_claude_code_runtime() {
        let outcome = slash_outcome("writing:manager", "claude-code", "t-");
        assert_eq!(
            outcome,
            SlashOutcome::Passthrough {
                session: "t-writing-manager".into(),
            }
        );
    }

    #[test]
    fn slash_outcome_honours_custom_tmux_prefix() {
        // `compose.global.supervisor.tmux_prefix` is operator-configurable.
        // The session formatter must concatenate verbatim — no hidden
        // dash-or-anything between prefix and project segment.
        let outcome = slash_outcome("news:head_editor", "claude-code", "a-");
        assert_eq!(
            outcome,
            SlashOutcome::Passthrough {
                session: "a-news-head_editor".into(),
            }
        );
    }

    #[test]
    fn slash_outcome_rejects_codex_runtime_with_named_runtime() {
        // Decision 6 ratify: non-CC managers reject slash-passthrough
        // and the rejection message must name the actual runtime so the
        // operator sees why nothing fired.
        let outcome = slash_outcome("writing:manager", "codex", "t-");
        let SlashOutcome::Reject { reason } = outcome else {
            panic!("non-CC runtime must reject");
        };
        assert!(
            reason.contains("Claude Code"),
            "rejection should reference Claude Code: {reason}"
        );
        assert!(
            reason.contains("codex"),
            "rejection should name the actual runtime: {reason}"
        );
    }

    #[test]
    fn slash_outcome_rejects_gemini_runtime_with_named_runtime() {
        let outcome = slash_outcome("writing:manager", "gemini", "t-");
        let SlashOutcome::Reject { reason } = outcome else {
            panic!("non-CC runtime must reject");
        };
        assert!(reason.contains("gemini"), "names the runtime: {reason}");
    }

    #[test]
    fn slash_outcome_rejects_malformed_manager_id() {
        // Defence in depth: if state.manager somehow lost the `:` (CLI
        // misuse, hand-edited env), refuse to type into a session
        // computed from a half-id rather than guess.
        let outcome = slash_outcome("not-a-manager-id", "claude-code", "t-");
        let SlashOutcome::Reject { reason } = outcome else {
            panic!("malformed manager id must reject");
        };
        assert!(reason.contains("malformed"), "names the failure: {reason}");
    }

    #[test]
    fn tmux_send_keys_argv_pins_send_keys_target_body_enter_shape() {
        // Pinning the argv shape so a future refactor that drops the
        // trailing literal `Enter` (which is what makes Claude Code
        // actually process the slash command) shows up as a test fail
        // rather than a silent passthrough that types but never submits.
        let argv = tmux_send_keys_argv("t-writing-manager", "/clear");
        assert_eq!(
            argv,
            ["send-keys", "-t", "t-writing-manager", "/clear", "Enter"]
        );
    }

    #[test]
    fn tmux_send_keys_argv_passes_body_verbatim_no_quote_munging() {
        // `Command::args` doesn't shell-quote — argv positions are passed
        // straight through. Tests pin that bodies with spaces / quotes
        // travel as a single arg without our code adding quoting that
        // tmux would then take literally.
        let argv = tmux_send_keys_argv("sess", "/compact focus on the cascade");
        assert_eq!(argv[3], "/compact focus on the cascade");
        assert_eq!(argv[4], "Enter");
    }

    // ── T-086-H setMyCommands registration ────────────────────────

    #[test]
    fn commands_for_runtime_returns_full_cc_list_for_claude_code() {
        let cmds = commands_for_runtime(Some("claude-code"));
        assert_eq!(
            cmds.len(),
            CC_SLASH_COMMANDS.len(),
            "CC manager registers the full curated list"
        );
        let names: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
        // Spot-check a few representative entries — adding/removing
        // entries from CC_SLASH_COMMANDS should consciously update
        // these spot-checks rather than silently drift.
        assert!(names.contains(&"clear"), "must include /clear: {names:?}");
        assert!(
            names.contains(&"compact"),
            "must include /compact: {names:?}"
        );
        assert!(names.contains(&"help"), "must include /help: {names:?}");
    }

    #[test]
    fn commands_for_runtime_returns_empty_for_codex() {
        // Decision 6 manager-only / CC-only routing: non-CC managers
        // register no autocomplete. Operator can still type slashes
        // manually but won't see the CC menu.
        assert!(commands_for_runtime(Some("codex")).is_empty());
    }

    #[test]
    fn commands_for_runtime_returns_empty_for_gemini() {
        assert!(commands_for_runtime(Some("gemini")).is_empty());
    }

    #[test]
    fn commands_for_runtime_returns_empty_for_unknown_runtime() {
        // Forward-compat: a future runtime ships before its
        // command-list does. The empty-list fallback means an old
        // team-bot binary against a new runtime degrades quietly.
        assert!(commands_for_runtime(Some("a-future-runtime")).is_empty());
    }

    #[test]
    fn commands_for_runtime_returns_empty_for_unscoped_bot() {
        // Unscoped bot (no `--manager`) → no runtime → no commands.
        // Slash-passthrough is gated on `state.manager.is_some()`
        // anyway, so the autocomplete would be misleading without it.
        assert!(commands_for_runtime(None).is_empty());
    }

    #[test]
    fn cc_slash_command_names_satisfy_telegram_constraints() {
        // Telegram restricts `BotCommand.command` to 1-32 chars,
        // lowercase letters / digits / underscores only. Pinning here
        // so a future CC slash-command-set update that adds a hyphen
        // (e.g. `output-style`) trips this test before hitting the
        // Telegram API and getting silently rejected.
        for (cmd, _desc) in CC_SLASH_COMMANDS {
            assert!(
                !cmd.is_empty() && cmd.len() <= 32,
                "command `{cmd}` violates 1-32 char limit"
            );
            assert!(
                cmd.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "command `{cmd}` contains chars Telegram rejects (only [a-z0-9_])"
            );
        }
    }

    #[test]
    fn cc_slash_command_descriptions_satisfy_telegram_constraints() {
        // Telegram requires `BotCommand.description` to be 3-256 chars.
        // Pinning here for the same reason as the command-name test.
        for (cmd, desc) in CC_SLASH_COMMANDS {
            assert!(
                desc.len() >= 3 && desc.len() <= 256,
                "description for `{cmd}` violates 3-256 char limit (got {} chars: {desc:?})",
                desc.len()
            );
        }
    }

    // ── T-086-E reaction dispatch ────────────────────────────────

    #[test]
    fn classify_kind_routes_reaction() {
        // T-086-E adds a fourth kind alongside text/image/file. Future
        // refactors that drop this arm should fail this test rather
        // than silently degrading reactions to UnknownFallback (which
        // would surface as text noise rather than an actual reaction).
        assert_eq!(classify_kind(Some("reaction")), DispatchKind::Reaction);
    }

    #[test]
    fn classify_kind_unknown_fallback_unchanged_for_other_strings() {
        // Regression guard: T-086-E must not accidentally turn the
        // `UnknownFallback` arm into a reaction match — only the
        // exact string "reaction" routes to the new arm.
        assert_eq!(
            classify_kind(Some("reactions")),
            DispatchKind::UnknownFallback
        );
        assert_eq!(classify_kind(Some("react")), DispatchKind::UnknownFallback);
    }

    #[test]
    fn parse_reaction_payload_extracts_telegram_msg_id_and_emoji() {
        let p = parse_reaction_payload(r#"{"telegram_msg_id":4242,"emoji":"👀"}"#)
            .expect("payload parses");
        assert_eq!(p.telegram_msg_id, 4242);
        assert_eq!(p.emoji, "👀");
    }

    #[test]
    fn parse_reaction_payload_returns_none_on_missing_fields() {
        assert!(parse_reaction_payload("not json").is_none());
        assert!(
            parse_reaction_payload(r#"{"emoji":"👍"}"#).is_none(),
            "missing telegram_msg_id"
        );
        assert!(
            parse_reaction_payload(r#"{"telegram_msg_id":7}"#).is_none(),
            "missing emoji"
        );
    }

    #[test]
    fn parse_reaction_payload_returns_none_on_wrong_types() {
        // Defence-in-depth: a malformed payload (string in place of
        // i64) shouldn't crash, just return None so the dispatcher
        // falls back to the log-and-skip arm.
        assert!(
            parse_reaction_payload(r#"{"telegram_msg_id":"oops","emoji":"👍"}"#).is_none(),
            "string telegram_msg_id"
        );
        assert!(
            parse_reaction_payload(r#"{"telegram_msg_id":7,"emoji":42}"#).is_none(),
            "non-string emoji"
        );
    }

    // ── T-102 typing-indicator helpers ───────────────────────────

    #[test]
    fn classify_kind_routes_typing() {
        // Sibling to the reaction dispatch test. A future refactor that
        // drops the `Some("typing")` arm degrades the indicator to
        // `UnknownFallback`, which would post the row's empty `text` to
        // the chat — we want that to fail this test rather than ship.
        assert_eq!(classify_kind(Some("typing")), DispatchKind::Typing);
    }

    #[test]
    fn classify_kind_unknown_fallback_unchanged_for_typing_lookalikes() {
        // Regression guard mirroring the reaction test: only the exact
        // string "typing" routes; "type" / "typings" stay in the
        // unknown bucket so a typo on the MCP side is loud, not silent.
        assert_eq!(classify_kind(Some("type")), DispatchKind::UnknownFallback);
        assert_eq!(
            classify_kind(Some("typings")),
            DispatchKind::UnknownFallback
        );
    }

    #[test]
    fn extend_typing_window_inserts_new_entry_with_deadline() {
        // First call for a chat: map gains an entry whose deadline is
        // exactly `now + ceiling`. The pure helper is the shared core
        // between the dispatcher and the refresh loop, so it has to
        // round-trip the math without rounding surprises.
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        let now = Instant::now();
        let ceiling = Duration::from_secs(10);
        let deadline = extend_typing_window(&mut map, ChatId(42), now, ceiling);
        assert_eq!(deadline, now + ceiling);
        assert_eq!(map.get(&ChatId(42)), Some(&(now + ceiling)));
    }

    #[test]
    fn extend_typing_window_resets_existing_deadline_on_second_call() {
        // Spec's "second call extends the window (resets the 10s
        // clock)": the new deadline is computed from the *new* `now`,
        // not appended to the old one. A monotonic clock makes "newer
        // is later" the correct invariant.
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        let t0 = Instant::now();
        let ceiling = Duration::from_secs(10);
        extend_typing_window(&mut map, ChatId(7), t0, ceiling);
        let t1 = t0 + Duration::from_secs(3);
        let deadline = extend_typing_window(&mut map, ChatId(7), t1, ceiling);
        assert_eq!(deadline, t1 + ceiling);
        assert_eq!(map.get(&ChatId(7)), Some(&(t1 + ceiling)));
    }

    #[test]
    fn clear_typing_window_removes_present_entry_and_reports_true() {
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        let now = Instant::now();
        extend_typing_window(&mut map, ChatId(1), now, Duration::from_secs(10));
        assert!(clear_typing_window(&mut map, ChatId(1)));
        assert!(!map.contains_key(&ChatId(1)));
    }

    #[test]
    fn clear_typing_window_returns_false_when_chat_not_tracked() {
        // A text/image/file dispatch happens whether or not a typing
        // window was open; the helper has to stay no-op-safe in the
        // absent case rather than panicking.
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        assert!(!clear_typing_window(&mut map, ChatId(99)));
    }

    #[test]
    fn refresh_typing_windows_drops_expired_and_returns_active() {
        // The refresh loop wakes every ~4s, drops anything past its
        // ceiling, and re-fires `sendChatAction` on whatever's left.
        // Pinning both halves of that: drop the expired entry, return
        // the still-active chats.
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        let now = Instant::now();
        map.insert(ChatId(1), now + Duration::from_secs(2));
        map.insert(ChatId(2), now - Duration::from_millis(10));
        let active = refresh_typing_windows(&mut map, now);
        assert_eq!(active, vec![ChatId(1)]);
        assert!(map.contains_key(&ChatId(1)));
        assert!(!map.contains_key(&ChatId(2)));
    }

    #[test]
    fn refresh_typing_windows_returns_empty_on_empty_map() {
        // Steady state: no agent has called `show_typing` recently, so
        // the refresh loop's tick produces no Telegram traffic.
        let mut map: HashMap<ChatId, Instant> = HashMap::new();
        let active = refresh_typing_windows(&mut map, Instant::now());
        assert!(active.is_empty());
    }

    // ── T-086-B reply_parameters dispatch ────────────────────────

    #[test]
    fn reply_parameters_for_returns_none_when_telegram_msg_id_is_none() {
        // Back-compat pin: rows without a threading target produce no
        // ReplyParameters so the dispatcher doesn't attach them and the
        // message lands as a fresh post.
        assert!(reply_parameters_for(None).is_none());
    }

    #[test]
    fn reply_parameters_for_returns_some_when_telegram_msg_id_is_set() {
        // Affirmative pin: present id → constructed `ReplyParameters`
        // ready for the teloxide builder. The actual MessageId carried
        // is asserted via the by-value PartialEq.
        let rp = reply_parameters_for(Some(12345)).expect("Some when set");
        assert_eq!(rp.message_id, MessageId(12345));
    }

    #[test]
    fn reply_parameters_for_safely_casts_within_i32_range() {
        // Telegram message ids are i32; Rust API takes i64 for SQLite
        // ergonomics. Pin that values comfortably within i32 range
        // round-trip exactly — guards against a future refactor that
        // drops the `as i32` cast and ends up with a wrap-around bug
        // on large but valid ids.
        let id: i64 = 2_000_000_000;
        let rp = reply_parameters_for(Some(id)).expect("Some when set");
        assert_eq!(rp.message_id, MessageId(id as i32));
    }

    // ── T-086-C inbound media helpers ─────────────────────────────

    #[test]
    fn extension_from_mime_covers_canonical_types() {
        assert_eq!(extension_from_mime("image/png"), "png");
        assert_eq!(extension_from_mime("image/jpeg"), "jpg");
        assert_eq!(extension_from_mime("image/webp"), "webp");
        assert_eq!(extension_from_mime("image/gif"), "gif");
        assert_eq!(extension_from_mime("application/pdf"), "pdf");
        assert_eq!(extension_from_mime("text/plain"), "txt");
        assert_eq!(extension_from_mime("application/zip"), "zip");
    }

    #[test]
    fn extension_from_mime_falls_back_to_bin_for_unknown() {
        // Forward-compat: a mime we haven't mapped still produces a real
        // file with a non-empty extension. The agent can re-mime via
        // libmagic if it cares; we don't pretend we know.
        assert_eq!(extension_from_mime("application/octet-stream"), "bin");
        assert_eq!(extension_from_mime("video/mp4"), "bin");
        assert_eq!(extension_from_mime(""), "bin");
    }

    #[test]
    fn extension_for_document_prefers_filename_extension() {
        // Filename's tail wins when it's a clean alphanumeric suffix —
        // even when it disagrees with the upload's mime type. Telegram
        // operators sometimes mislabel; trust the filename they typed.
        assert_eq!(
            extension_for_document(Some("report.pdf"), "application/octet-stream"),
            "pdf"
        );
        assert_eq!(
            extension_for_document(Some("snapshot.PNG"), "application/pdf"),
            "png",
            "case-folded to lowercase"
        );
    }

    #[test]
    fn extension_for_document_falls_back_to_mime_when_filename_missing() {
        assert_eq!(extension_for_document(None, "image/png"), "png");
        assert_eq!(
            extension_for_document(None, "application/octet-stream"),
            "bin"
        );
    }

    #[test]
    fn extension_for_document_rejects_funky_extensions() {
        // No extension → mime fallback.
        assert_eq!(extension_for_document(Some("README"), "text/plain"), "txt");
        // Empty extension after dot → mime fallback.
        assert_eq!(
            extension_for_document(Some("trailing."), "text/plain"),
            "txt"
        );
        // Non-alphanumeric chars → mime fallback (defends against weird
        // shell/fs metacharacters that could foul a path).
        assert_eq!(
            extension_for_document(Some("name.weird/ext"), "image/png"),
            "png"
        );
        // Over-long extension → mime fallback (sanity cap on what we'd
        // accept verbatim from a user-controlled filename).
        assert_eq!(
            extension_for_document(Some("name.thisistoolongatail"), "image/png"),
            "png"
        );
    }

    #[test]
    fn inbound_media_path_composes_root_project_rowid_extension() {
        let root = std::path::Path::new("/srv/.team/state/inbound-media");
        let path = inbound_media_path(root, "writing", 42, "jpg");
        assert_eq!(
            path,
            std::path::PathBuf::from("/srv/.team/state/inbound-media/writing/42.jpg")
        );
    }

    #[test]
    fn media_success_payload_includes_path_mime_size_and_omits_empty_caption() {
        let path = std::path::Path::new("/srv/.team/state/inbound-media/p/7.jpg");
        let s = media_success_payload(path, "", "image/jpeg", 1024);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["path"], "/srv/.team/state/inbound-media/p/7.jpg");
        assert_eq!(v["mime"], "image/jpeg");
        assert_eq!(v["size_bytes"], 1024);
        assert!(
            v.get("caption").is_none(),
            "empty caption omitted from payload"
        );
    }

    #[test]
    fn media_success_payload_includes_caption_when_present() {
        let path = std::path::Path::new("/x.png");
        let s = media_success_payload(path, "look at this", "image/png", 32);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["caption"], "look at this");
    }

    #[test]
    fn media_error_payload_carries_verbose_error_and_optional_caption() {
        // R12: media_error rows must surface the verbatim cause so the
        // agent can ack to the user with a real diagnostic.
        let s = media_error_payload("", "get_file: timed out");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["error"], "get_file: timed out");
        assert!(v.get("caption").is_none());

        let s = media_error_payload("a screenshot", "create file: permission denied");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["error"], "create file: permission denied");
        assert_eq!(v["caption"], "a screenshot");
    }

    #[test]
    fn placeholder_then_success_update_round_trip() {
        // Pin the two-phase SQL pattern: insert a `media_pending` row,
        // then UPDATE to `image` with the final payload. The kind +
        // structured_payload must reflect the final state and the row
        // id must remain stable so the on-disk filename matches.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, kind, structured_payload)
             VALUES ('p', 'user:telegram', 'p:eng_lead', 'cap',
                     strftime('%s','now'), 'media_pending', '{}')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();

        // Simulated post-download UPDATE.
        let payload =
            media_success_payload(std::path::Path::new("/x/p/3.jpg"), "cap", "image/jpeg", 128);
        conn.execute(
            "UPDATE messages SET kind = ?1, structured_payload = ?2 WHERE id = ?3",
            params!["image", payload, id],
        )
        .unwrap();

        let (kind, sp): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, structured_payload FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind.as_deref(), Some("image"));
        let v: serde_json::Value = serde_json::from_str(sp.as_deref().unwrap()).unwrap();
        assert_eq!(v["mime"], "image/jpeg");
    }

    #[test]
    fn placeholder_then_error_update_writes_media_error_kind() {
        // Mirror of the success path for the failure mode (R12). After
        // the UPDATE the row's kind is `media_error` and the payload
        // carries the verbatim cause — operator's reply prompt has the
        // diagnostic in hand.
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, kind, structured_payload)
             VALUES ('p', 'user:telegram', 'p:eng_lead', '',
                     strftime('%s','now'), 'media_pending', '{}')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();

        let payload = media_error_payload("", "download_file: 502 bad gateway");
        conn.execute(
            "UPDATE messages SET kind = 'media_error', structured_payload = ?1 WHERE id = ?2",
            params![payload, id],
        )
        .unwrap();

        let (kind, sp): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, structured_payload FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind.as_deref(), Some("media_error"));
        assert!(sp.unwrap().contains("502 bad gateway"));
    }

    // ── T-101 voice STT mapping ────────────────────────────────

    #[test]
    fn map_voice_outcome_ok_yields_quoted_reply_and_prefixed_inbox_row() {
        let d = map_voice_outcome(&SttOutcome::Ok("hello team".into()));
        assert_eq!(d.user_reply, "🎙 \"hello team\"");
        assert_eq!(
            d.inbox_text.as_deref(),
            Some("🎙 (transcribed voice, may have misspellings): hello team")
        );
        // Pin the prefix constant so future renames flag through this test.
        assert!(d
            .inbox_text
            .as_deref()
            .unwrap()
            .starts_with(VOICE_INBOX_PREFIX));
    }

    #[test]
    fn map_voice_outcome_skipped_yields_no_inbox_row() {
        let d = map_voice_outcome(&SttOutcome::Skipped);
        assert!(d.inbox_text.is_none());
        // Skipped vs Failed must stay distinct (per issue, conflating
        // them is a UX bug). The skipped phrasing asks if the operator
        // said something — failed messages name the failure.
        assert!(d.user_reply.contains("couldn't capture anything"));
        assert!(!d.user_reply.contains("failed"));
    }

    #[test]
    fn map_voice_outcome_failed_yields_no_inbox_row_and_surfaces_error() {
        let d = map_voice_outcome(&SttOutcome::Failed("network down".into()));
        assert!(d.inbox_text.is_none());
        assert!(d.user_reply.contains("failed"));
        assert!(d.user_reply.contains("network down"));
        // And the failure shape must NOT match the skipped phrasing —
        // the operator needs to know whether they were heard or not.
        assert!(!d.user_reply.contains("couldn't capture"));
    }

    #[test]
    fn voice_inbox_prefix_matches_issue_spec() {
        // Pin the exact string the issue spec calls for, so a future
        // rename can't silently drift the model-facing contract.
        assert_eq!(
            VOICE_INBOX_PREFIX,
            "🎙 (transcribed voice, may have misspellings):"
        );
    }

    // ── T-236 voice-STT-missing config hint ────────────────────────

    #[test]
    fn voice_stt_missing_reply_carries_operator_actionable_hints() {
        // Done-when contract (issue #236): when voice arrives on a
        // manager-scoped bot without STT configured, the reply must
        // convey (a) confirmation the voice was received (operator's
        // primary confusion), (b) why nothing's happening, (c) two
        // clear paths to fix it — `/teamctl:adjust` (conversational)
        // AND manual project YAML — and (d) a docs pointer. Pin each
        // piece so wording can evolve without dropping a contract.
        let body = voice_stt_missing_reply();
        assert!(
            body.starts_with("🎙"),
            "reply must lead with the voice glyph so the operator sees \
             this is about their voice message: {body}"
        );
        assert!(
            body.contains("Voice isn't configured"),
            "reply must name the cause so the operator knows what to fix: {body}"
        );
        assert!(
            body.contains("/teamctl:adjust"),
            "reply must surface the conversational fix path: {body}"
        );
        assert!(
            body.contains("interfaces.telegram.speech_to_text"),
            "reply must surface the YAML key for the manual fix path: {body}"
        );
        assert!(
            body.contains("https://teamctl.run/"),
            "reply must include a docs pointer the operator can open: {body}"
        );
    }

    // ── #279: voice-download size ceiling ──────────────────────────

    #[test]
    fn voice_size_pre_reject_names_both_numbers() {
        // Operator-visible error must surface both the reported size
        // and the ceiling so the cause is obvious without log-diving.
        let msg = voice_size_pre_reject(5_000_000, MAX_VOICE_BYTES);
        assert!(msg.contains("5000000"), "msg names reported: {msg}");
        assert!(
            msg.contains(&MAX_VOICE_BYTES.to_string()),
            "msg names max: {msg}"
        );
        assert!(msg.contains("too large"), "msg flags the rejection: {msg}");
    }

    #[test]
    fn voice_path_mid_reject_names_both_numbers_and_distinguishes() {
        // The voice-path BoundedWriter constructs its mid-reject via
        // `media_size_mid_reject("voice file", ...)` — verify that
        // call shape still produces a distinguishable message naming
        // both numbers (#279 invariant, kept by the #332 generalization
        // of the helper).
        let msg = media_size_mid_reject("voice file", 1_000_000, 1_500_000, MAX_VOICE_BYTES);
        assert!(msg.contains("voice file"), "msg carries voice kind: {msg}");
        assert!(msg.contains("2500000"), "msg names cumulative: {msg}");
        assert!(
            msg.contains(&MAX_VOICE_BYTES.to_string()),
            "msg names max: {msg}"
        );
        assert!(
            msg.contains("mid-download"),
            "msg distinguishes from pre-reject: {msg}"
        );
    }

    #[tokio::test]
    async fn bounded_writer_passes_through_when_under_max() {
        use tokio::io::AsyncWriteExt;
        let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 16);
        bw.write_all(b"hello").await.unwrap();
        bw.write_all(b" world").await.unwrap();
        bw.flush().await.unwrap();
        assert_eq!(bw.into_inner(), b"hello world");
    }

    #[tokio::test]
    async fn bounded_writer_allows_exactly_max() {
        // Boundary: cumulative `max` bytes succeed; only `max + 1`
        // trips the abort. Mirrors the strict `>` comparison.
        use tokio::io::AsyncWriteExt;
        let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 5);
        bw.write_all(b"hello").await.unwrap();
        assert_eq!(bw.into_inner(), b"hello");
    }

    #[tokio::test]
    async fn bounded_writer_errors_when_chunk_would_exceed_max() {
        use tokio::io::AsyncWriteExt;
        let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 4);
        // First write fits exactly.
        bw.write_all(b"abcd").await.unwrap();
        // Second write would push past — must error, must not append.
        let err = bw.write_all(b"e").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("mid-download"),
            "error must use media_size_mid_reject format: {err}"
        );
        assert_eq!(
            bw.into_inner().as_slice(),
            b"abcd",
            "rejected chunk must not append"
        );
    }

    #[tokio::test]
    async fn bounded_writer_errors_on_single_oversize_chunk() {
        // A single write larger than `max` must also abort, with
        // nothing written — covers a lying upstream that dumps the
        // whole body in one chunk.
        use tokio::io::AsyncWriteExt;
        let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 4);
        let err = bw.write_all(b"abcde").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("mid-download"));
        assert!(
            bw.into_inner().is_empty(),
            "no bytes must land when the chunk overshoots"
        );
    }

    // ── #332: disk-fill defense on the `download_to` path ──────────

    #[test]
    fn media_size_pre_reject_carries_kind_and_names_both_numbers() {
        // The `kind` label distinguishes voice (#279, RAM-OOM) from
        // media (#332, disk-fill) so operators can tell which boundary
        // fired without log-diving.
        let msg = media_size_pre_reject("media file", 60_000_000, MAX_DOWNLOAD_BYTES);
        assert!(msg.contains("media file"), "msg carries kind: {msg}");
        assert!(msg.contains("60000000"), "msg names reported: {msg}");
        assert!(
            msg.contains(&MAX_DOWNLOAD_BYTES.to_string()),
            "msg names max: {msg}"
        );
        assert!(msg.contains("too large"), "msg flags the rejection: {msg}");
    }

    #[test]
    fn media_size_mid_reject_carries_kind_and_distinguishes() {
        let msg = media_size_mid_reject("media file", 40_000_000, 20_000_000, MAX_DOWNLOAD_BYTES);
        assert!(msg.contains("media file"), "msg carries kind: {msg}");
        assert!(msg.contains("60000000"), "msg names cumulative: {msg}");
        assert!(
            msg.contains(&MAX_DOWNLOAD_BYTES.to_string()),
            "msg names max: {msg}"
        );
        assert!(
            msg.contains("mid-download"),
            "msg distinguishes from pre-reject: {msg}"
        );
    }

    #[tokio::test]
    async fn bounded_writer_aborts_mid_stream_on_tokio_fs_file_without_excess_disk_bytes() {
        // #332 disk-path pin: `BoundedWriter` wrapping a real
        // `tokio::fs::File` aborts the over-cap chunk WITHOUT having
        // written it to disk. Proves the streaming-abort defense holds
        // at the actual sink `download_to` uses, not just the `Vec<u8>`
        // sink from #279 — the `>` check fires before
        // `inner.poll_write`, so no disk write occurs for the rejected
        // chunk.
        use tokio::io::AsyncWriteExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media");
        let f = tokio::fs::File::create(&path).await.unwrap();
        let mut bw = BoundedWriter::new("media file", f, 4);
        bw.write_all(b"abcd").await.unwrap();
        let err = bw.write_all(b"e").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("media file"),
            "mid-reject must carry the `media file` kind: {err}"
        );
        let mut f = bw.into_inner();
        f.flush().await.ok();
        drop(f);
        let bytes = tokio::fs::read(&path).await.unwrap();
        assert_eq!(
            bytes, b"abcd",
            "rejected chunk must not have hit disk — bounded handle saw \
             the over-cap chunk and aborted before `inner.poll_write`"
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn max_download_bytes_strictly_exceeds_max_voice_bytes() {
        // Bump-policy invariant: the disk path accepts strictly larger
        // files than the voice path (different threat model — disk
        // tolerates more than RAM-OOM). Catches an accidental swap or
        // misordering of the constants. The assertion is on consts by
        // design (that's the invariant); clippy's `assertions_on_
        // constants` would flag it otherwise.
        assert!(
            MAX_DOWNLOAD_BYTES > MAX_VOICE_BYTES,
            "MAX_DOWNLOAD_BYTES ({MAX_DOWNLOAD_BYTES}) must exceed \
             MAX_VOICE_BYTES ({MAX_VOICE_BYTES})"
        );
    }
}
