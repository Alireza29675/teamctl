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
    }
}
