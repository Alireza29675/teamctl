use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

mod cmd;
mod managed_bot;
mod term;

/// Shared project + per-agent scope for `up` / `down` / `reload`
/// (T-305). Flattened into each so `--help` documents the same four
/// scope levels everywhere. clap enforces: the positional agent list
/// and `--except` are mutually exclusive, and both require `<PROJECT>`.
#[derive(Args, Debug)]
struct AgentScope {
    /// Project to scope to — matches `project.id` or the project
    /// filename without `.yaml`. Omit to act on every project.
    #[arg(value_name = "PROJECT")]
    project: Option<String>,

    /// Restrict the action to these agents within <PROJECT>. The rest
    /// of the project is left alone. Requires <PROJECT>; mutually
    /// exclusive with --except.
    #[arg(value_name = "AGENT", requires = "project")]
    agents: Vec<String>,

    /// Act on every agent in <PROJECT> EXCEPT the named one(s).
    /// Repeatable. Requires <PROJECT>; mutually exclusive with the
    /// positional agent list.
    #[arg(
        long = "except",
        value_name = "AGENT",
        requires = "project",
        conflicts_with = "agents"
    )]
    except: Vec<String>,
}

#[derive(Parser)]
#[command(
    name = "teamctl",
    version = env!("TEAMCTL_BUILD_VERSION"),
    about = "Declarative CLI for persistent AI agent teams",
    long_about = None,
    help_template = "\
{name} {version}
{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}",
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
        /// Template name. Known: `guided`, `essentials`, `blank`. With
        /// `--yes`, defaults to `essentials`; without, the interactive
        /// picker defaults to `guided`. `--template guided --yes` is
        /// an error — `guided` requires interactive confirmation.
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

    /// Confirm intent, then exec `claude /teamctl:adjust` to evolve an
    /// existing `.team/` (hire, retire, modify, bot-setup, add project,
    /// open a bridge). Interactive-only — the skill collects what it
    /// needs from there.
    Adjust {
        /// Reserved for parity with other `teamctl` subcommands.
        /// Rejected at runtime — the underlying skill is interactive
        /// top-to-bottom, so `--yes` cannot meaningfully skip it.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    // ── Lifecycle ────────────────────────────────────────────────────
    /// Parse the compose tree and check invariants.
    Validate,
    /// Render artifacts and start agents' tmux sessions.
    ///
    /// Scope levels: no args → every project · `<project>` → every
    /// agent in it · `<project> <agent>…` → just those agents ·
    /// `<project> --except <agent>…` → all but those. Already-running
    /// agents are a no-op.
    Up {
        #[command(flatten)]
        scope: AgentScope,
        /// Start agents in a brand-new Claude conversation (re-runs the
        /// bootstrap prompt) instead of resuming the prior session.
        /// Durable on-disk files (task.md, memory, ways-of-working) are
        /// kept. Only affects agents this command actually starts;
        /// already-running agents are untouched. Claude runtime only.
        #[arg(long)]
        fresh: bool,
    },
    /// Stop agents' tmux sessions. State is preserved.
    ///
    /// Scope levels: no args → every project · `<project>` → every
    /// agent in it · `<project> <agent>…` → just those agents ·
    /// `<project> --except <agent>…` → all but those.
    Down {
        #[command(flatten)]
        scope: AgentScope,
    },
    /// Apply compose changes. Unscoped: restarts changed agents only.
    ///
    /// Scope levels: no args → every project · `<project>` → every
    /// agent in it · `<project> <agent>…` → just those agents ·
    /// `<project> --except <agent>…` → all but those. A per-agent
    /// scope FORCE-restarts the selected agents even if their config
    /// is unchanged; unscoped reload keeps the changed-only behavior
    /// unless `--force` is passed.
    Reload {
        /// Print the reload plan without rendering, registering, or
        /// touching any agent. Same per-line format as a real reload,
        /// annotated with `(dry run)`.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        scope: AgentScope,
        /// Restart agents into a brand-new Claude conversation (re-runs
        /// the bootstrap prompt) instead of resuming the prior session.
        /// Durable on-disk files are kept. Applies to the agents this
        /// reload restarts; to fresh-restart specific unchanged agents
        /// use the scoped form `reload <project> <agent> --fresh`.
        /// Composes with `--force`. Claude runtime only.
        #[arg(long)]
        fresh: bool,
        /// Restart every agent in scope even when its config is
        /// unchanged. Without this, an unscoped reload restarts only
        /// agents whose env, mcp, or role_prompt changed. Composes with
        /// `--fresh` and `--dry-run`. (A per-agent scope already
        /// force-restarts, so `--force` is for the unscoped and
        /// `<project>`-only forms.)
        #[arg(long)]
        force: bool,
    },

    // ── Inspection ───────────────────────────────────────────────────
    /// Wide table: agents, supervisor state, inbox depth.
    #[command(alias = "status")]
    Ps,
    /// Host-wide list of every teamctl-managed tmux session, across
    /// projects (one row per project). With no subcommand, lists.
    /// Use `teamctl sessions kill <project>` to tear down every
    /// session belonging to one project — the only path that works
    /// for orphans, where the compose file is gone and `teamctl down`
    /// can't reach the project. No compose root required.
    Sessions {
        #[command(subcommand)]
        action: Option<SessionsAction>,
        /// (No subcommand) emit machine-readable JSON instead of the
        /// human table.
        #[arg(long)]
        json: bool,
    },
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
    /// Attach to an agent's tmux session (read-write by default).
    Attach {
        target: String,
        /// Attach read-only — observe the session without sending keystrokes.
        #[arg(long)]
        ro: bool,
        /// Accepted for back-compat; read-write is now the default (no-op).
        #[arg(long, hide = true)]
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

    // ── Self-update ─────────────────────────────────────────────────
    /// Update teamctl in place by re-running the installer that brought
    /// it in. Detects shell-installer / Homebrew / cargo automatically;
    /// override with `--method`.
    Update {
        /// Force a particular install method (`shell` · `brew` · `cargo`).
        #[arg(long)]
        method: Option<String>,
        /// Print the version comparison and exit; don't update.
        #[arg(long)]
        check: bool,
        /// Skip the "Proceed?" confirmation.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Print the GitHub release body for a teamctl version (defaults
    /// to the running binary's version). Same renderer as the
    /// post-update display; non-conforming bodies fall through to raw.
    /// With `--since <ver>`, prints every release body in (ver, current]
    /// at or above the curated-display floor.
    Whatsnew {
        /// Version to look up (e.g. `0.7.3`). When omitted, uses the
        /// running binary's version. Ignored when `--since` is set.
        version: Option<String>,
        /// Aggregate mode: print every release body strictly newer than
        /// `<ver>` up to and including the running binary's version,
        /// oldest-first, with one framed range header.
        #[arg(long)]
        since: Option<String>,
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

    /// Record a rate-limit hit for <agent>. Invoked by the StopFailure
    /// hook; not normally run by hand.
    #[command(name = "rl-hit")]
    RlHit { agent: String },

    /// Record one budget cost row for <agent> from the Stop hook payload on
    /// stdin. Invoked by the Stop hook; not normally run by hand.
    #[command(name = "budget-record")]
    BudgetRecord { agent: String },
}

#[derive(Subcommand)]
enum SessionsAction {
    /// Kill every teamctl-managed tmux session for the named project.
    /// The orphan case (compose gone) is the only one `teamctl down`
    /// can't handle, which is the entire reason this exists. Kills
    /// are reversible via `teamctl up`; no confirmation prompt.
    Kill { project: String },
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
    if let Command::Adjust { yes } = cli.command {
        return cmd::adjust::run(yes);
    }
    if let Command::Ui { no_prompt, argv } = cli.command {
        return cmd::ui::run(no_prompt, argv);
    }
    if let Command::Update { method, check, yes } = cli.command {
        return cmd::update::run(method, check, yes);
    }
    if let Command::Whatsnew { version, since } = cli.command {
        return cmd::whatsnew::run(version, since);
    }
    if let Command::Sessions { action, json } = cli.command {
        return match action {
            None => cmd::sessions::run(json),
            Some(SessionsAction::Kill { project }) => cmd::sessions::kill(&project),
        };
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
        Command::Up { scope, fresh } => {
            let sel = cmd::agent_filter::AgentSelector::from_args(scope.agents, scope.except);
            cmd::up::run(&root, scope.project.as_deref(), &sel, fresh)
        }
        Command::Down { scope } => {
            let sel = cmd::agent_filter::AgentSelector::from_args(scope.agents, scope.except);
            cmd::down::run(&root, scope.project.as_deref(), &sel)
        }
        Command::Reload {
            dry_run,
            scope,
            fresh,
            force,
        } => {
            let sel = cmd::agent_filter::AgentSelector::from_args(scope.agents, scope.except);
            cmd::reload::run(&root, dry_run, scope.project.as_deref(), &sel, fresh, force)
        }
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
        Command::RlHit { agent } => cmd::rl_hit::run(&root, &agent),
        Command::BudgetRecord { agent } => cmd::budget_record::run(&root, &agent),
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
        Command::Attach { target, ro, rw: _ } => cmd::attach::run(&root, &target, ro),
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
        Command::Adjust { .. } => unreachable!("handled above"),
        Command::Sessions { .. } => unreachable!("handled above"),
        Command::Ui { .. } => unreachable!("handled above"),
        Command::Update { .. } => unreachable!("handled above"),
        Command::Whatsnew { .. } => unreachable!("handled above"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse an argv into the `Attach` variant or panic with the clap
    /// error — keeps the per-case asserts focused on the flags.
    fn parse_attach(args: &[&str]) -> (String, bool, bool) {
        let cli = Cli::try_parse_from(args).expect("attach args parse");
        match cli.command {
            Command::Attach { target, ro, rw } => (target, ro, rw),
            _ => panic!("expected the Attach subcommand"),
        }
    }

    // #385: read-write is the default — bare `teamctl attach <id>` must
    // leave `ro` false so keystrokes reach the agent.
    #[test]
    fn attach_defaults_to_read_write() {
        let (target, ro, rw) = parse_attach(&["teamctl", "attach", "t-demo-mgr"]);
        assert_eq!(target, "t-demo-mgr");
        assert!(!ro, "attach must default to read-write");
        assert!(!rw, "--rw not passed, so its flag stays false");
    }

    // `--ro` opts into read-only.
    #[test]
    fn attach_ro_flag_sets_read_only() {
        let (_, ro, _) = parse_attach(&["teamctl", "attach", "t-demo-mgr", "--ro"]);
        assert!(ro, "--ro must request a read-only attach");
    }

    // `--rw` is hidden back-compat: it still parses (so old muscle memory
    // and scripts don't error) but is ignored — `ro` stays false and
    // main.rs binds `rw: _`.
    #[test]
    fn attach_rw_flag_still_parses_and_is_ignored() {
        let (target, ro, rw) = parse_attach(&["teamctl", "attach", "t-demo-mgr", "--rw"]);
        assert_eq!(target, "t-demo-mgr");
        assert!(rw, "--rw still parses for back-compat");
        assert!(!ro, "--rw does not flip on read-only");
    }

    // `--ro --rw` together is accepted (no clap conflict) and resolves to
    // read-only: `ro` wins and main.rs ignores `rw` (binds `rw: _`).
    #[test]
    fn attach_ro_and_rw_together_is_read_only() {
        let (_, ro, rw) = parse_attach(&["teamctl", "attach", "t-demo-mgr", "--ro", "--rw"]);
        assert!(ro, "--ro requests read-only even when --rw is also present");
        assert!(rw, "--rw still parses alongside --ro");
    }
}
