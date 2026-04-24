use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "teamctl",
    version,
    about = "Declarative CLI for persistent AI agent teams",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Parse the compose tree and validate invariants.
    Validate {
        #[arg(default_value = ".")]
        path: String,
    },
    /// Bring the fleet up.
    Up,
    /// Bring the fleet down. State is preserved.
    Down,
    /// Apply compose changes by diffing against the last-applied snapshot.
    Reload,
    /// Print a table of agents and their states.
    Status,
    /// Tail journal + per-agent logs.
    Logs { target: String },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAMCTL_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        None => {
            println!(
                "teamctl {} — run `teamctl --help` for usage.",
                team_core::VERSION
            );
        }
        Some(Command::Validate { path }) => {
            println!("validate: {path} (stub — implemented in Phase 1)");
        }
        Some(Command::Up) => println!("up: stub (Phase 1)"),
        Some(Command::Down) => println!("down: stub (Phase 1)"),
        Some(Command::Reload) => println!("reload: stub (Phase 1)"),
        Some(Command::Status) => println!("status: stub (Phase 1)"),
        Some(Command::Logs { target }) => println!("logs {target}: stub (Phase 1)"),
    }
    Ok(())
}
