//! `teamctl bot` — set up and supervise 1:1 Telegram bots, one per
//! user-facing manager.
//!
//! `bot setup` walks the operator through BotFather → token → `/start`
//! → chat id, lets them pick env-var names (sensible defaults), writes
//! the values into `.team/.env`, and upserts a `telegram:` block into
//! the manager definition in `projects/<id>.yaml`. After setup,
//! `teamctl up` spawns one `team-bot` per manager-with-`telegram` so
//! the human DMs the manager's bot directly.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use team_core::compose::Compose;

use crate::managed_bot::ManagedBotClient;

pub fn run(root: &Path, action: BotAction) -> Result<()> {
    match action {
        BotAction::Setup { force, manager } => setup(root, force, manager),
        BotAction::List => list(root),
        BotAction::Status => status(root),
    }
}

#[derive(Debug)]
pub enum BotAction {
    Setup {
        force: bool,
        manager: Option<String>,
    },
    List,
    Status,
}

mod config;
mod legacy;
mod prompts;
mod reconcile;
mod status;
mod supervisor;
mod telegram;
mod wizard;

use config::*;
use prompts::*;
pub(crate) use reconcile::{plan_bots, start_bots, stop_bots};
pub use status::bot_session_name;
use status::*;
#[allow(unused_imports)] // Public facade preserves the existing helper types.
pub use supervisor::{
    bot_specs, down_one, down_orphans, team_bot_bin, up_one, BotSpec, BotSttSpec,
};

pub(crate) fn persisted_sessions(
    root: &Path,
) -> Result<std::collections::BTreeMap<String, String>> {
    supervisor::load_sessions(root)
}
use telegram::*;
use wizard::*;

#[cfg(test)]
mod tests;
