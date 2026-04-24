//! `team-mcp` — the shared agent mailbox, exposed as an MCP stdio server.
//!
//! Protocol: MCP (JSON-RPC 2.0 over stdio, line-delimited). Protocol version
//! pinned to the workspace constant `team_core::MCP_PROTOCOL_VERSION`.
//!
//! Each agent runs its own `team-mcp` child (spawned by the runtime via
//! `--mcp-config`). All processes point at the same SQLite file. Concurrent
//! writers are handled by SQLite in WAL mode; inotify + polling wake
//! `inbox_watch` listeners within milliseconds.

mod rpc;
mod store;
mod tools;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<rpc::Request>(trimmed) {
            Ok(req) => {
                if let Some(resp) = rpc::dispatch(&ctx, req).await {
                    let mut buf = serde_json::to_vec(&resp)?;
                    buf.push(b'\n');
                    stdout.write_all(&buf).await?;
                    stdout.flush().await?;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, line = %trimmed, "invalid JSON-RPC");
                let resp = rpc::Response::parse_error(&e.to_string());
                let mut buf = serde_json::to_vec(&resp)?;
                buf.push(b'\n');
                stdout.write_all(&buf).await?;
                stdout.flush().await?;
            }
        }
    }
    Ok(())
}
