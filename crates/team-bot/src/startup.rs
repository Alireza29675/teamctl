use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;
use teloxide::prelude::*;
use tokio::sync::Mutex;

use crate::{
    approval::handle_callback,
    config::{parse_authorized_chats, Cli},
    inbound::handle_message,
    outbound::{outbound_loop, typing_refresh_loop},
    repository::{agent_runtime, open_mailbox},
    tmux::commands_for_runtime,
    State, SttRuntime,
};

pub(super) async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_BOT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let token = cli.resolved_token()?;
    let stt_key = cli.resolved_stt_key()?;
    let bot = Bot::new(token);
    let conn = open_mailbox(&cli.mailbox)?;
    let allow = parse_authorized_chats(cli.authorized_chat_ids.as_deref())?;
    let media_root = cli
        .mailbox
        .parent()
        .map(|p| p.join("inbound-media"))
        .unwrap_or_else(|| PathBuf::from("inbound-media"));
    // T-101: build the STT runtime only when all three required flags
    // (provider, key, model) are present. Partial config is treated as
    // "no voice" rather than an error — `teamctl bot up` already prints a
    // skip line when the API key env var is unset.
    let stt = match (cli.stt_provider, stt_key, cli.stt_model) {
        (Some(provider), Some(api_key), Some(model)) => Some(SttRuntime {
            provider,
            api_key,
            model,
            language: cli.stt_language,
            http: reqwest::Client::new(),
        }),
        _ => None,
    };
    if let Some(id) = &cli.interface_id {
        let agents = cli.manager.iter().cloned().collect::<Vec<_>>();
        let existing = team_core::interfaces::get(&conn, id)?;
        let info = team_core::interfaces::InterfaceInfo {
            id: id.clone(),
            provider: "telegram".into(),
            name: cli
                .interface_name
                .clone()
                .or_else(|| existing.as_ref().map(|i| i.name.clone()))
                .unwrap_or_else(|| id.clone()),
            description: cli
                .interface_description
                .clone()
                .or_else(|| existing.as_ref().map(|i| i.description.clone()))
                .unwrap_or_default(),
            capabilities: ["text", "image", "file", "typing", "reaction"]
                .map(str::to_string)
                .to_vec(),
        };
        team_core::interfaces::register(&conn, &info, &agents)?;
    }
    let state = Arc::new(State {
        compose_root: cli.compose_root,
        interface_id: cli.interface_id,
        conn: Mutex::new(conn),
        allow,
        manager: cli.manager,
        manager_display_name: cli.manager_display_name,
        tmux_prefix: cli.tmux_prefix,
        media_root,
        stt,
        typing: Mutex::new(HashMap::new()),
        rich: !cli.no_rich_messages,
    });
    {
        let conn = state.conn.lock().await;
        crate::chat::register_capabilities(&conn, &state.adapter_id())?;
    }

    // T-086-H: register the manager's runtime-appropriate slash commands
    // with Telegram so the operator gets autocomplete on `/`. Manager-scoped
    // CC and Codex bots register their curated command lists; other runtimes
    // and unscoped bots register nothing (clean degrade per Decision 6). The
    // registration is best-effort — a Telegram API error is logged but
    // doesn't abort startup, since slash-passthrough (PR-G) still works
    // when the operator types the chord manually.
    let runtime = if let Some(mgr) = state.manager.as_deref() {
        let c = state.conn.lock().await;
        agent_runtime(&c, mgr)
    } else {
        None
    };
    let commands = if state.scoped_native()? {
        vec![
            teloxide::types::BotCommand::new("help", "Show messaging commands"),
            teloxide::types::BotCommand::new("pending", "Show pending approvals"),
        ]
    } else {
        commands_for_runtime(runtime.as_deref())
    };
    if !commands.is_empty() {
        if let Err(e) = bot.set_my_commands(commands).await {
            tracing::warn!(
                "set_my_commands failed (operator gets no autocomplete; \
                 slash-passthrough still works manually): {e}"
            );
        }
    }

    // Conversation replies use their recorded peer; approvals and legacy
    // mailbox rows retain the primary-chat compatibility fallback.
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
