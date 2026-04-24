use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(
    name = "teamctl",
    version,
    about = "Declarative CLI for persistent AI agent teams",
    long_about = None,
)]
struct Cli {
    /// Compose root (directory containing `team-compose.yaml`).
    #[arg(long, short = 'C', env = "TEAMCTL_ROOT", default_value = ".")]
    root: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse the compose tree and check invariants.
    Validate,
    /// Render artifacts and start every agent's tmux session.
    Up,
    /// Stop every agent's tmux session. State is preserved.
    Down,
    /// Apply compose changes. Restarts changed agents only.
    Reload,
    /// Print agents, supervisor state, inbox depth.
    Status,
    /// Tail logs for one agent (tmux pipe-pane).
    Logs {
        /// Target id as `<project>:<agent>`.
        target: String,
    },
    /// Inject a message as `sender=cli`.
    Send {
        /// Recipient `<project>:<agent>`.
        target: String,
        /// Message text.
        text: String,
    },
    /// Manage inter-project manager bridges.
    Bridge {
        #[command(subcommand)]
        action: BridgeAction,
    },
    /// Show pending HITL approval requests.
    Pending,
    /// Approve a pending HITL request.
    Approve {
        id: i64,
        #[arg(long)]
        note: Option<String>,
    },
    /// Deny a pending HITL request.
    Deny {
        id: i64,
        #[arg(long)]
        note: Option<String>,
    },
    /// Per-project activity and cost for today.
    Budget {
        #[arg(long)]
        project: Option<String>,
    },
    /// Garbage-collect expired messages and stale approvals.
    Gc,
}

#[derive(Subcommand)]
enum BridgeAction {
    /// Open a new bridge between two managers in different projects.
    Open {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        topic: String,
        /// TTL in minutes. Default 120.
        #[arg(long, default_value_t = 120)]
        ttl: u64,
    },
    /// Close a bridge by id.
    Close { id: i64 },
    /// List bridges (open, expired, closed).
    List,
    /// Print the transcript for a bridge.
    Log { id: i64 },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAMCTL_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let root = cli
        .root
        .canonicalize()
        .with_context(|| format!("canonicalize --root {}", cli.root.display()))?;

    match cli.command {
        Command::Validate => cmd::validate::run(&root),
        Command::Up => cmd::up::run(&root),
        Command::Down => cmd::down::run(&root),
        Command::Reload => cmd::reload::run(&root),
        Command::Status => cmd::status::run(&root),
        Command::Logs { target } => cmd::logs::run(&root, &target),
        Command::Send { target, text } => cmd::send::run(&root, &target, &text),
        Command::Budget { project } => cmd::budget::run(&root, project.as_deref()),
        Command::Gc => cmd::gc::run(&root),
        Command::Pending => cmd::approval::pending(&root),
        Command::Approve { id, note } => cmd::approval::decide(&root, id, true, note.as_deref()),
        Command::Deny { id, note } => cmd::approval::decide(&root, id, false, note.as_deref()),
        Command::Bridge { action } => match action {
            BridgeAction::Open {
                from,
                to,
                topic,
                ttl,
            } => cmd::bridge::open(&root, &from, &to, &topic, ttl),
            BridgeAction::Close { id } => cmd::bridge::close(&root, id),
            BridgeAction::List => cmd::bridge::list(&root),
            BridgeAction::Log { id } => cmd::bridge::log(&root, id),
        },
    }
}
