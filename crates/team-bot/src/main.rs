//! `team-bot` — Telegram adapter for the teamctl `interfaces:` abstraction.
//!
//! Watches the mailbox for messages addressed to managers with
//! `telegram_inbox: true` and for new pending approvals, and surfaces both to
//! the authorized Telegram chat. Inbound user messages (DMs + callback
//! button taps) write back into the mailbox.
//!
//! Later interface adapters (`team-interface-discord`, `-imessage`, `-cli`)
//! mirror this crate's shape: an async loop against the same SQLite mailbox
//! plus an adapter-specific transport.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup};
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
}

struct State {
    conn: Mutex<Connection>,
    allow: Vec<i64>,
    /// `<project>:<manager>` if this instance is scoped; otherwise all managers.
    manager: Option<String>,
}

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
    let state = Arc::new(State {
        conn: Mutex::new(conn),
        allow,
        manager: cli.manager,
    });

    // Outbound: poll approvals + mailbox, surface to primary chat.
    {
        let bot = bot.clone();
        let state = state.clone();
        tokio::spawn(async move { outbound_loop(bot, state).await });
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
    if let Some(rest) = trimmed.strip_prefix("/dm ") {
        if let Some((target, body)) = rest.split_once(' ') {
            if let Some((project, _)) = target.split_once(':') {
                let c = state.conn.lock().await;
                let _ = c.execute(
                    "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
                     VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'))",
                    params![project, target, body],
                );
                drop(c);
                bot.send_message(msg.chat.id, format!("→ {target}")).await?;
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
                    "#{id} {agent} · {action}: {}\n",
                    render_plain(&summary)
                ));
            }
            bot.send_message(msg.chat.id, out).await?;
        }
    } else if trimmed == "/start" || trimmed == "/help" {
        bot.send_message(
            msg.chat.id,
            "teamctl — Telegram interface\n\
             /dm <project>:<agent> <message> — send a DM\n\
             /pending — show pending approvals",
        )
        .await?;
    }
    Ok(())
}

async fn handle_callback(bot: Bot, q: CallbackQuery, state: Arc<State>) -> ResponseResult<()> {
    let chat_id = q.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    if !state.is_authorized(chat_id) {
        return Ok(());
    }
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some((verb, id_str)) = data.split_once(':') else {
        return Ok(());
    };
    let Ok(id) = id_str.parse::<i64>() else {
        return Ok(());
    };
    let approved = verb == "approve";

    // Atomic decision: only update if still pending. Returned row count tells
    // us whether this tap was the live decision or a stale duplicate.
    let decided_now = {
        let c = state.conn.lock().await;
        let _ = c.execute(
            "UPDATE approvals SET delivered_at=strftime('%s','now')
             WHERE id=?1 AND delivered_at IS NULL",
            params![id],
        );
        c.execute(
            "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'), decided_by='user:telegram'
             WHERE id=?2 AND status='pending'",
            params![if approved { "approved" } else { "denied" }, id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
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
        let outcome = if approved {
            "✅ Approved by Alireza"
        } else {
            "❌ Rejected by Alireza"
        };
        let new_text = if original.is_empty() {
            outcome.to_string()
        } else {
            format!("{original}\n\n{outcome}")
        };
        let _ = bot.edit_message_text(chat, mid, new_text).await;
        let _ = bot
            .edit_message_reply_markup(chat, mid)
            .reply_markup(InlineKeyboardMarkup::new(Vec::<Vec<_>>::new()))
            .await;
    }

    bot.answer_callback_query(q.id)
        .text(format!("{} #{id}", if approved { "✅" } else { "❌" }))
        .await?;
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
        let approvals: Vec<(i64, String, String, String)> = {
            let c = state.conn.lock().await;
            let rows: Vec<(i64, String, String, String)> = match state.manager_project() {
                Some(project) => {
                    let mut stmt = c
                        .prepare(
                            "SELECT id, agent_id, action, summary FROM approvals
                             WHERE status='pending' AND id > ?1 AND project_id = ?2
                             ORDER BY id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_approval_id, project], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
                None => {
                    let mut stmt = c
                        .prepare(
                            "SELECT id, agent_id, action, summary FROM approvals
                             WHERE status='pending' AND id > ?1 ORDER BY id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_approval_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
            };
            rows
        };
        for (id, agent, action, summary) in approvals {
            last_approval_id = last_approval_id.max(id);
            // T-027: when scoped to a manager, only surface approvals filed by
            // agents that report up to *this* bot's manager. With a manager
            // bot per tier (eng_lead, pm) Alireza sees one prompt per agent.
            if let Some(scoped) = state.manager.as_deref() {
                let routed = {
                    let c = state.conn.lock().await;
                    manager_of(&c, &agent).unwrap_or_else(|| agent.clone())
                };
                if routed != scoped {
                    continue;
                }
            }
            let kb = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("Approve", format!("approve:{id}")),
                InlineKeyboardButton::callback("Deny", format!("deny:{id}")),
            ]]);
            let text = format!(
                "🔐 #{id}  {agent}\naction: {action}\n{}",
                render_plain(&summary)
            );
            let send_ok = bot.send_message(chat, text).reply_markup(kb).await.is_ok();
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
        // tool inserts rows with `recipient = 'user:telegram'`; in scoped
        // mode we only forward replies from the configured manager's project.
        let forwardable: Vec<(i64, String, String)> = {
            let c = state.conn.lock().await;
            let rows: Vec<(i64, String, String)> = match state.manager_project() {
                Some(project) => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.text FROM messages m
                             WHERE m.id > ?1
                               AND m.recipient = 'user:telegram'
                               AND m.acked_at IS NULL
                               AND m.project_id = ?2
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id, project], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
                None => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.text FROM messages m
                             WHERE m.id > ?1
                               AND m.recipient = 'user:telegram'
                               AND m.acked_at IS NULL
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
            };
            rows
        };
        for (id, sender, text) in forwardable {
            last_msg_id = last_msg_id.max(id);
            let _ = bot
                .send_message(chat, format!("[{sender}] {}", render_plain(&text)))
                .await;
            let c = state.conn.lock().await;
            let _ = c.execute(
                "UPDATE messages SET acked_at = strftime('%s','now') WHERE id = ?1",
                params![id],
            );
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

/// Strip lightweight markdown so Telegram renders clean prose with emoji
/// accents instead of literal `**bold**` / `_italic_` / `- bullet` syntax.
/// We deliberately do not translate to MarkdownV2 — Alireza prefers plain
/// text, and stripping is failure-mode-symmetric (no escaping landmines).
fn render_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (idx, line) in s.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
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
        out.push_str(&strip_inline_markdown(&body));
    }
    out
}

/// Drop `**`, `__`, single `*` / `_` emphasis, and inline-code backticks.
/// Keeps URL text intact (we never see `[label](url)` rendered as a link
/// anyway in plain Telegram messages).
fn strip_inline_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '*' || c == '_') && chars.peek() == Some(&c) {
            // Paired `**` / `__` emphasis → drop both.
            chars.next();
            continue;
        }
        if c == '*' || c == '_' || c == '`' {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

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

    #[test]
    fn render_plain_strips_paired_emphasis() {
        assert_eq!(render_plain("**bold** text"), "bold text");
        assert_eq!(render_plain("__also bold__"), "also bold");
        assert_eq!(render_plain("plain `code` here"), "plain code here");
    }

    #[test]
    fn render_plain_strips_single_emphasis() {
        assert_eq!(render_plain("*italic* text"), "italic text");
        assert_eq!(render_plain("_underscored_"), "underscored");
    }

    #[test]
    fn render_plain_translates_list_bullets() {
        let input = "- one\n- two\n  * nested\n+ three";
        let expected = "• one\n• two\n  • nested\n• three";
        assert_eq!(render_plain(input), expected);
    }

    #[test]
    fn render_plain_preserves_emoji_and_plain_prose() {
        let input = "🔐 deploy\nrouting prompt to one channel — the **right** one";
        let expected = "🔐 deploy\nrouting prompt to one channel — the right one";
        assert_eq!(render_plain(input), expected);
    }
}
