use clap::Parser;

#[derive(Parser)]
#[command(
    name = "team-mcp",
    version,
    about = "MCP server exposing the shared agent mailbox"
)]
struct Cli {
    /// Fully-qualified agent id as `<project>:<agent>`.
    #[arg(long, env = "TEAMCTL_AGENT_ID")]
    agent_id: Option<String>,

    /// Path to the SQLite mailbox database.
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    mailbox: Option<String>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_MCP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    tracing::info!(
        agent_id = ?cli.agent_id,
        mailbox = ?cli.mailbox,
        version = team_core::VERSION,
        "team-mcp starting (Phase 0 stub)"
    );
    // Phase 1 implements the JSON-RPC 2.0 stdio loop and tool dispatch.
    Ok(())
}
