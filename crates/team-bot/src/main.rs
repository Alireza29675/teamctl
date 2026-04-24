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

    /// Comma-separated list of authorized chat ids. Required.
    #[arg(long, env = "TEAMCTL_TELEGRAM_CHATS", value_delimiter = ',')]
    authorized_chat_ids: Vec<i64>,

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
    let state = Arc::new(State {
        conn: Mutex::new(conn),
        allow: cli.authorized_chat_ids,
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
    conn.execute_batch(team_core::mailbox::SCHEMA)?;
    Ok(conn)
}

async fn handle_message(bot: Bot, msg: Message, state: Arc<State>) -> ResponseResult<()> {
    if !state.is_authorized(msg.chat.id.0) {
        return Ok(());
    }
    let Some(text) = msg.text() else {
        return Ok(());
    };
    let trimmed = text.trim();
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
                out.push_str(&format!("#{id} {agent} · {action}: {summary}\n"));
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
    let Some(data) = q.data else { return Ok(()) };
    if let Some((verb, id_str)) = data.split_once(':') {
        if let Ok(id) = id_str.parse::<i64>() {
            let approved = verb == "approve";
            let c = state.conn.lock().await;
            let _ = c.execute(
                "UPDATE approvals SET status=?1, decided_at=strftime('%s','now'), decided_by='user:telegram'
                 WHERE id=?2 AND status='pending'",
                params![if approved { "approved" } else { "denied" }, id],
            );
            drop(c);
            bot.answer_callback_query(q.id)
                .text(format!("{} {id}", if approved { "✅" } else { "❌" }))
                .await?;
        }
    }
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
            let kb = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("Approve", format!("approve:{id}")),
                InlineKeyboardButton::callback("Deny", format!("deny:{id}")),
            ]]);
            let text = format!("🔐 #{id}  {agent}\naction: {action}\n{summary}");
            let _ = bot.send_message(chat, text).reply_markup(kb).await;
        }

        let forwardable: Vec<(i64, String, String, String)> = {
            let c = state.conn.lock().await;
            let rows: Vec<(i64, String, String, String)> = match state.manager.as_deref() {
                Some(mgr) => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.recipient, m.text FROM messages m
                             JOIN agents a ON a.id = m.recipient
                             WHERE m.id > ?1
                               AND m.sender != 'user:telegram'
                               AND m.acked_at IS NULL
                               AND a.is_manager = 1
                               AND a.id = ?2
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id, mgr], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
                None => {
                    let mut stmt = c
                        .prepare(
                            "SELECT m.id, m.sender, m.recipient, m.text FROM messages m
                             JOIN agents a ON a.id = m.recipient
                             WHERE m.id > ?1
                               AND m.sender != 'user:telegram'
                               AND m.acked_at IS NULL
                               AND a.is_manager = 1
                             ORDER BY m.id",
                        )
                        .unwrap();
                    stmt.query_map(params![last_msg_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap()
                    .flatten()
                    .collect()
                }
            };
            rows
        };
        for (id, sender, recipient, text) in forwardable {
            last_msg_id = last_msg_id.max(id);
            let _ = bot
                .send_message(chat, format!("→ {recipient}\n(from {sender})\n{text}"))
                .await;
        }
    }
}

async fn current_max(state: &Arc<State>, table: &str) -> i64 {
    let sql = format!("SELECT COALESCE(MAX(id), 0) FROM {table}");
    let c = state.conn.lock().await;
    c.query_row(&sql, [], |r| r.get(0)).unwrap_or(0)
}
