//! `team-mcp` — the shared agent mailbox, exposed as an MCP stdio server.
//!
//! Protocol: MCP (JSON-RPC 2.0 over stdio, line-delimited). Protocol version
//! pinned to the workspace constant `team_core::MCP_PROTOCOL_VERSION`.
//!
//! Each agent runs its own `team-mcp` child (spawned by the runtime via
//! `--mcp-config`). All processes point at the same SQLite file. Concurrent
//! writers are handled by SQLite in WAL mode.
//!
//! # Channels delivery
//!
//! When the connected client is Claude Code v2.1.80+ launched with
//! `--channels server:team`, this server pushes every new inbox row as an
//! `notifications/claude/channel` JSON-RPC notification. The runtime injects
//! the payload into the live session as a `<channel source="team">` event,
//! so the agent reacts on arrival without polling. The watcher initialises
//! its high-water mark to the current max inbox id at startup, so unacked
//! pre-existing mail is left for the agent to fetch via `inbox_peek` (the
//! bootstrap prompt directs it to). Other runtimes silently ignore the
//! notification, so emitting unconditionally is safe.

mod rpc;
mod store;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use tokio::sync::{Mutex, Notify};

#[derive(Parser)]
#[command(
    name = "team-mcp",
    version,
    about = "MCP server for the teamctl mailbox"
)]
struct Cli {
    /// Fully-qualified agent id as `<project>:<agent>`.
    #[arg(long, env = "TEAMCTL_AGENT_ID")]
    agent_id: String,

    /// Path to the SQLite mailbox database.
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    mailbox: PathBuf,
}

/// Poll cadence for the channel watcher. SQLite SELECT against a WAL-mode
/// db with one indexed predicate is sub-millisecond; 500 ms is the
/// "feels instant to a human, costs ~nothing" sweet spot.
const CHANNEL_POLL_MS: u64 = 500;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_MCP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let store = store::Store::open(&cli.mailbox)?;
    let ctx = tools::Ctx::new(cli.agent_id.clone(), store);

    tracing::info!(
        agent_id = %cli.agent_id,
        mailbox = %cli.mailbox.display(),
        "team-mcp ready",
    );

    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    // Channels notifications must not be sent before the client signals
    // `notifications/initialized` per the MCP lifecycle. The watcher waits
    // on this gate; the request loop trips it.
    let initialized = Arc::new(Notify::new());

    spawn_channel_watcher(
        ctx.store.clone(),
        ctx.agent_id.clone(),
        stdout.clone(),
        initialized.clone(),
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<rpc::Request>(trimmed) {
            Ok(req) => {
                if req.method == "notifications/initialized" {
                    initialized.notify_waiters();
                }
                if let Some(resp) = rpc::dispatch(&ctx, req).await {
                    let buf = serde_json::to_vec(&resp)?;
                    write_line(&stdout, &buf).await?;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, line = %trimmed, "invalid JSON-RPC");
                let resp = rpc::Response::parse_error(&e.to_string());
                let buf = serde_json::to_vec(&resp)?;
                write_line(&stdout, &buf).await?;
            }
        }
    }
    Ok(())
}

async fn write_line(stdout: &Arc<Mutex<Stdout>>, buf: &[u8]) -> Result<()> {
    let mut out = stdout.lock().await;
    out.write_all(buf).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

fn spawn_channel_watcher(
    store: Arc<store::Store>,
    agent_id: String,
    stdout: Arc<Mutex<Stdout>>,
    initialized: Arc<Notify>,
) {
    tokio::spawn(async move {
        initialized.notified().await;

        // High-water mark: messages with `id <= last_seen` were already
        // present at session start. The bootstrap prompt directs the agent
        // to fetch those via `inbox_peek` for catch-up, so we don't push
        // them as channel events (which would race the agent's own peek).
        let mut last_seen: i64 = match store.inbox_peek(&agent_id, 1000) {
            Ok(msgs) => msgs.iter().map(|m| m.id).max().unwrap_or(0),
            Err(e) => {
                tracing::warn!(error = %e, "channel watcher initial peek failed");
                0
            }
        };

        loop {
            tokio::time::sleep(Duration::from_millis(CHANNEL_POLL_MS)).await;
            let msgs = match store.inbox_peek(&agent_id, 100) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "channel watcher peek failed");
                    continue;
                }
            };
            let mut max_id = last_seen;
            for m in msgs.iter().filter(|m| m.id > last_seen) {
                let payload = format_channel_event(m);
                let buf = match serde_json::to_vec(&payload) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "channel notification serialise failed");
                        continue;
                    }
                };
                if let Err(e) = write_line(&stdout, &buf).await {
                    tracing::warn!(error = %e, "channel notification write failed; aborting watcher");
                    return;
                }
                if m.id > max_id {
                    max_id = m.id;
                }
            }
            last_seen = max_id;
        }
    });
}

/// Build the JSON-RPC notification per Claude Code's Channels wire format.
/// `meta` fields surface as XML attributes on the `<channel>` tag the
/// runtime injects into the session.
fn format_channel_event(m: &store::Message) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": {
            "content": m.text,
            "meta": {
                "id": m.id,
                "sender": m.sender,
                "recipient": m.recipient,
                "thread_id": m.thread_id,
                "sent_at": m.sent_at,
            }
        }
    })
}
