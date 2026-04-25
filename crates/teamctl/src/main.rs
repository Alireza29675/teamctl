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
    /// Compose root (the directory holding `team-compose.yaml`). When unset,
    /// teamctl walks up from CWD looking for `.team/team-compose.yaml`.
    #[arg(long, short = 'C', env = "TEAMCTL_ROOT")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // ── Setup ────────────────────────────────────────────────────────
    /// Scaffold a fresh `.team/` directory in the current repo.
    Init {
        /// Template name. Use `--list` to see options.
        #[arg(long)]
        template: Option<String>,
        /// Project id (default: derived from the repo directory name).
        #[arg(long)]
        project: Option<String>,
        /// Skip prompts; accept defaults.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    // ── Lifecycle ────────────────────────────────────────────────────
    /// Parse the compose tree and check invariants.
    Validate,
    /// Render artifacts and start every agent's tmux session.
    Up,
    /// Stop every agent's tmux session. State is preserved.
    Down,
    /// Apply compose changes. Restarts changed agents only.
    Reload,

    // ── Inspection ───────────────────────────────────────────────────
    /// Wide table: agents, supervisor state, inbox depth.
    #[command(alias = "status")]
    Ps,
    /// Tail an agent's tmux pane scrollback.
    Logs { target: String },
    /// Live message stream for an agent (-f to follow).
    Tail {
        target: String,
        #[arg(short, long)]
        follow: bool,
    },
    /// Inbox snapshot for an agent (or `--all`).
    Mail {
        target: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Full snapshot of an agent: env, mcp, prompt, recent messages, costs.
    Inspect { target: String },

    // ── Mailbox ──────────────────────────────────────────────────────
    /// Inject a message as `sender=cli`.
    Send { target: String, text: String },

    // ── Approvals ────────────────────────────────────────────────────
    /// Show pending HITL approval requests.
    #[command(alias = "pending")]
    Approvals,
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

    // ── Bridges ──────────────────────────────────────────────────────
    /// Manage inter-project manager bridges.
    Bridge {
        #[command(subcommand)]
        action: BridgeAction,
    },

    // ── Budget / GC ─────────────────────────────────────────────────
    /// Per-project activity and cost for today.
    Budget {
        #[arg(long)]
        project: Option<String>,
    },
    /// Garbage-collect expired messages and stale approvals.
    Gc,

    // ── Attach / exec ────────────────────────────────────────────────
    /// Attach to an agent's tmux session (read-only by default).
    Attach {
        target: String,
        /// Allow keyboard input. Dangerous — confirms before attaching.
        #[arg(long)]
        rw: bool,
    },
    /// Run a command in an agent's CWD with its env loaded.
    Exec {
        target: String,
        #[arg(last = true, allow_hyphen_values = true, num_args = 1..)]
        argv: Vec<String>,
    },
    /// Open an interactive shell in an agent's CWD with its env loaded.
    Shell { target: String },

    // ── Env / context (no compose root required) ─────────────────────
    /// List or doctor the environment variables referenced by compose.
    Env {
        #[arg(long)]
        doctor: bool,
    },
    /// Switch between named `.team/` roots on this machine.
    Context {
        #[command(subcommand)]
        action: ContextAction,
    },

    // ── Internal ────────────────────────────────────────────────────
    /// Wrap a runtime invocation, watching for rate-limit signatures.
    /// Used by `agent-wrapper.sh`; not normally invoked by hand.
    #[command(name = "rl-watch")]
    RlWatch {
        target: String,
        #[arg(last = true, allow_hyphen_values = true)]
        runtime_command: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ContextAction {
    /// List registered contexts.
    Ls,
    /// Print the active context name.
    Current,
    /// Set the active context.
    Use { name: String },
    /// Register a new context.
    Add { name: String, path: PathBuf },
    /// Remove a context.
    Rm { name: String },
}

#[derive(Subcommand)]
enum BridgeAction {
    Open {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        topic: String,
        #[arg(long, default_value_t = 120)]
        ttl: u64,
    },
    Close {
        id: i64,
    },
    #[command(alias = "list")]
    Ls,
    Log {
        id: i64,
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

    // Some commands don't need a resolved root — handle them up front.
    if let Command::Init {
        template,
        project,
        yes,
    } = cli.command
    {
        return cmd::init::run(template, project, yes);
    }
    if let Command::Context { action } = &cli.command {
        return match action {
            ContextAction::Ls => cmd::context::ls(),
            ContextAction::Current => cmd::context::current(),
            ContextAction::Use { name } => cmd::context::use_(name),
            ContextAction::Add { name, path } => cmd::context::add(name, path),
            ContextAction::Rm { name } => cmd::context::rm(name),
        };
    }

    let root = resolve_root(cli.root)?;

    match cli.command {
        Command::Validate => cmd::validate::run(&root),
        Command::Up => {
            let r = cmd::up::run(&root);
            // Auto-register the context on first up.
            let _ = cmd::context::auto_register(&root);
            r
        }
        Command::Down => cmd::down::run(&root),
        Command::Reload => cmd::reload::run(&root),
        Command::Ps => cmd::status::run(&root),
        Command::Logs { target } => cmd::logs::run(&root, &target),
        Command::Tail { target, follow } => cmd::tail::run(&root, &target, follow),
        Command::Mail { target, all } => cmd::mail::run(&root, target.as_deref(), all),
        Command::Inspect { target } => cmd::inspect::run(&root, &target),
        Command::Send { target, text } => cmd::send::run(&root, &target, &text),
        Command::Budget { project } => cmd::budget::run(&root, project.as_deref()),
        Command::Gc => cmd::gc::run(&root),
        Command::RlWatch {
            target,
            runtime_command,
        } => cmd::rl_watch::run(&root, &target, &runtime_command),
        Command::Approvals => cmd::approval::pending(&root),
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
            BridgeAction::Ls => cmd::bridge::list(&root),
            BridgeAction::Log { id } => cmd::bridge::log(&root, id),
        },
        Command::Attach { target, rw } => cmd::attach::run(&root, &target, rw),
        Command::Exec { target, argv } => cmd::exec::run(&root, &target, &argv),
        Command::Shell { target } => cmd::exec::shell(&root, &target),
        Command::Env { doctor } => cmd::env::run(&root, doctor),
        Command::Context { .. } => unreachable!("handled above"),
        Command::Init { .. } => unreachable!("handled above"),
    }
}

/// Resolution order: `--root` flag > `TEAMCTL_ROOT` env > current context >
/// walk up from CWD looking for `.team/`.
fn resolve_root(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return p
            .canonicalize()
            .with_context(|| format!("canonicalize --root {}", p.display()));
    }
    if let Some(p) = cmd::context::root_for_current()? {
        return Ok(p);
    }
    let cwd = std::env::current_dir().context("get cwd")?;
    team_core::compose::Compose::discover(&cwd)
}
