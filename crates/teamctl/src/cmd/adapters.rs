//! External messaging adapters use a versioned process boundary.

mod assets;
pub(crate) mod config;
mod host;
pub(crate) mod lifecycle;
mod process;
mod service;

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use team_core::chat::protocol::Mode;

#[derive(Subcommand)]
pub enum Action {
    /// List declared messaging adapters and their process state.
    List,
    /// Install the pinned optional WhatsApp adapter and its dependencies.
    Setup { name: String },
    /// Pair an adapter interactively; QR material is never saved by the host.
    Pair { name: String },
    /// Start only this adapter's managed host.
    Start { name: String },
    /// Stop only this adapter, preserving the rest of the team.
    Stop { name: String },
    /// Inspect incoming opaque identities without forwarding messages.
    Identify { name: String },
    /// Run a declared adapter host in the foreground.
    Run {
        name: String,
        #[arg(long, hide = true)]
        context_file: Option<PathBuf>,
    },
}

pub fn run(root: &Path, action: Action) -> Result<()> {
    super::up::source_dotenv_into_process(root);
    let compose = team_core::compose::Compose::load_read_only(root)?;
    match action {
        Action::List => {
            for adapter in config::specs(&compose)? {
                let state = lifecycle::observed(&compose, &adapter)?;
                println!(
                    "{}  {}  {}  {:?}",
                    adapter.name, adapter.kind, adapter.config.agent, state
                );
            }
            for bot in super::bot::bot_specs(&compose) {
                println!(
                    "telegram:{}  telegram  {}  native",
                    bot.manager, bot.manager
                );
            }
            println!("local  cli/ui  operator  available");
            Ok(())
        }
        Action::Setup { name } => {
            let spec = config::find(&compose, &name)?;
            anyhow::ensure!(spec.kind == "whatsapp", "setup is supplied for the WhatsApp adapter; install a custom adapter using its own instructions");
            assets::install(&compose.root)
        }
        Action::Run { name, context_file } => {
            let spec = config::find(&compose, &name).context("adapter configuration")?;
            service::run_with_context(&compose, &spec, Mode::Run, false, context_file.as_deref())
        }
        Action::Identify { name } => {
            use std::io::IsTerminal;
            anyhow::ensure!(
                std::io::stdout().is_terminal(),
                "identity inspection needs an interactive terminal"
            );
            let spec = config::find(&compose, &name)?;
            service::run_with_context(&compose, &spec, Mode::Run, true, None)
        }
        Action::Start { name } => {
            let _identity_lock = team_core::registry::lifecycle_lock()?;
            super::up::guard_no_name_collision(&compose, None)?;
            let _root_lock = team_core::persistence::ProcessLock::acquire(
                &compose.root.join("state/lifecycle.lock.sqlite"),
            )?;
            super::up::reserve_in_registry(&compose, None)?;
            lifecycle::start_one(&compose, &name)
        }
        Action::Stop { name } => {
            let _identity_lock = team_core::registry::lifecycle_lock()?;
            super::up::guard_no_name_collision(&compose, None)?;
            let _root_lock = team_core::persistence::ProcessLock::acquire(
                &compose.root.join("state/lifecycle.lock.sqlite"),
            )?;
            lifecycle::stop_one(&compose, &name)
        }
        Action::Pair { name } => {
            use std::io::IsTerminal;
            anyhow::ensure!(
                std::io::stdout().is_terminal(),
                "pairing needs an interactive terminal; QR secrets cannot be piped to a log"
            );
            let spec = config::find(&compose, &name).context("adapter configuration")?;
            service::run(&compose, &spec, Mode::Pair)
        }
    }
}
