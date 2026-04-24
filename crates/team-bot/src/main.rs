use clap::Parser;

#[derive(Parser)]
#[command(name = "team-bot", version, about = "Telegram bot for teamctl")]
struct Cli {
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    mailbox: Option<String>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_BOT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    tracing::info!(mailbox = ?cli.mailbox, "team-bot starting (Phase 0 stub — Phase 6 adds teloxide)");
    Ok(())
}
