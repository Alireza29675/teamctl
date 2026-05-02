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
    //
    // Note: `TEAMCTL_ROOT` is read manually in `resolve_root_with_source` so
    // we can distinguish a CLI-supplied `--root` from an env-supplied one
    // (T-010: env-as-source emits a stderr warning, CLI does not).
    #[arg(long, short = 'C')]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // ── Setup ────────────────────────────────────────────────────────
    /// Scaffold a fresh `.team/` directory.
    ///
    /// With `name`: creates `<name>/<name>/.team/...` so a fresh
    /// `cd <name> && teamctl up` Just Works. Without `name`: scaffolds
    /// `.team/` in the current directory.
    Init {
        /// Folder name to create. Doubles as the default project id.
        /// When omitted, scaffolds `.team/` directly in cwd.
        name: Option<String>,
        /// Template name. Defaults to `solo`. Known: `solo`, `blank`.
        #[arg(long)]
        template: Option<String>,
        /// Project id override (default: derived from `name` or cwd).
        #[arg(long)]
        project: Option<String>,
        /// Overwrite an existing `.team/` at the target path.
        #[arg(long)]
        force: bool,
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
    Reload {
        /// Print the reload plan without rendering, registering, or
        /// touching any agent. Same per-line format as a real reload,
        /// annotated with `(dry run)`.
        #[arg(long)]
        dry_run: bool,
    },

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

    // ── TUI ──────────────────────────────────────────────────────────
    /// Launch the `teamctl-ui` TUI. If `teamctl-ui` is on PATH, exec
    /// to it with any extra args forwarded; if not, print an install
    /// hint and (interactively) offer to run `cargo install teamctl-ui`.
    Ui {
        /// Skip the install prompt; just print the hint and exit.
        /// Implicit when stdin is non-interactive (CI / pipes).
        #[arg(long)]
        no_prompt: bool,
        /// Args forwarded to `teamctl-ui`. Use `--` to separate them
        /// from teamctl's own flags: `teamctl ui -- --root /path`.
        #[arg(last = true, allow_hyphen_values = true)]
        argv: Vec<String>,
    },

    // ── Telegram bots ────────────────────────────────────────────────
    /// Set up and inspect 1:1 manager↔Telegram bots.
    Bot {
        #[command(subcommand)]
        action: BotAction,
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
enum BotAction {
    /// Interactive wizard: walks BotFather → token → /start → chat id,
    /// writes env vars to `.team/.env`, and adds an
    /// `interfaces.telegram` block to the manager in
    /// `projects/<id>.yaml`. Resumable — re-runs only ask for what's
    /// still missing.
    Setup {
        /// Optional `<project>:<role>` to scope the wizard to one
        /// manager. When omitted, walks every manager and skips ones
        /// already fully wired up.
        manager: Option<String>,
        /// Re-run setup even when env vars are already populated
        /// (re-asks for token + chat id).
        #[arg(long)]
        force: bool,
    },
    /// Print every manager that has an `interfaces.telegram` block
    /// with env-var status.
    #[command(alias = "ls")]
    List,
    /// Show running/stopped tmux session for each bot.
    Status,
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
        name,
        template,
        project,
        force,
        yes,
    } = cli.command
    {
        return cmd::init::run(name, template, project, force, yes);
    }
    if let Command::Ui { no_prompt, argv } = cli.command {
        return cmd::ui::run(no_prompt, argv);
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

    let (root, source) = resolve_root_with_source(cli.root)?;

    // T-010: warn when an introspection command resolves a root that didn't
    // come from CWD walk-up or an explicit `--root`. Read-side only — write
    // commands have a different blast-radius story (see T-010b).
    let warns_on_override = matches!(
        cli.command,
        Command::Validate | Command::Ps | Command::Mail { .. } | Command::Inspect { .. }
    );
    if warns_on_override {
        cmd::warn::maybe_warn_root_source(&source, &root);
    }

    match cli.command {
        Command::Validate => cmd::validate::run(&root),
        Command::Up => cmd::up::run(&root),
        Command::Down => cmd::down::run(&root),
        Command::Reload { dry_run } => cmd::reload::run(&root, dry_run),
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
        Command::Bot { action } => {
            let action = match action {
                BotAction::Setup { force, manager } => {
                    cmd::bot::BotAction::Setup { force, manager }
                }
                BotAction::List => cmd::bot::BotAction::List,
                BotAction::Status => cmd::bot::BotAction::Status,
            };
            cmd::bot::run(&root, action)
        }
        Command::Context { .. } => unreachable!("handled above"),
        Command::Init { .. } => unreachable!("handled above"),
        Command::Ui { .. } => unreachable!("handled above"),
    }
}

/// Resolve the compose root and report which input it came from. Resolution
/// order: `--root` flag > `TEAMCTL_ROOT` env > walk up from CWD looking for
/// `.team/`. T-008 removed the registered-context fallback — operators must
/// `cd` into a tree containing `.team/` or pass `-C <path>`. The returned
/// [`cmd::warn::RootSource`] drives the T-010 override-warning on read-side
/// commands.
fn resolve_root_with_source(explicit: Option<PathBuf>) -> Result<(PathBuf, cmd::warn::RootSource)> {
    use cmd::warn::RootSource;

    if let Some(p) = explicit {
        let canon = p
            .canonicalize()
            .with_context(|| format!("canonicalize --root {}", p.display()))?;
        return Ok((canon, RootSource::CliFlag));
    }
    if let Some(raw) = std::env::var_os("TEAMCTL_ROOT") {
        // Treat `TEAMCTL_ROOT=""` (exported empty) the same as unset —
        // canonicalize would error otherwise, hiding the real "no root"
        // diagnostic the walk-up path produces.
        if !raw.is_empty() {
            let p = PathBuf::from(raw);
            let canon = p
                .canonicalize()
                .with_context(|| format!("canonicalize $TEAMCTL_ROOT {}", p.display()))?;
            return Ok((canon, RootSource::Env));
        }
    }
    let cwd = std::env::current_dir().context("get cwd")?;
    let p = team_core::compose::Compose::discover(&cwd)?;
    Ok((p, RootSource::WalkUp))
}
