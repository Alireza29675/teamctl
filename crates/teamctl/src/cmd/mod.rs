pub mod approval;
pub mod attach;
pub mod bridge;
pub mod budget;
pub mod context;
pub mod down;
pub mod env;
pub mod exec;
pub mod gc;
pub mod init;
pub mod inspect;
pub mod logs;
pub mod mail;
pub mod reload;
pub mod rl_watch;
pub mod send;
pub mod snapshot;
pub mod status;
pub mod tail;
pub mod up;
pub mod validate;
pub mod warn;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use team_core::compose::Compose;

pub fn load(root: &Path) -> Result<Compose> {
    Compose::load(root).with_context(|| format!("load compose at {}", root.display()))
}

/// Absolute path to the colocated `team-mcp` binary. Resolution order:
/// 1. `$TEAMCTL_TEAM_MCP` env override.
/// 2. Sibling of the running `teamctl` executable.
/// 3. `team-mcp` on `$PATH`.
pub fn team_mcp_bin() -> PathBuf {
    if let Ok(p) = std::env::var("TEAMCTL_TEAM_MCP") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join(if cfg!(windows) {
                "team-mcp.exe"
            } else {
                "team-mcp"
            });
            if c.exists() {
                return c;
            }
        }
    }
    PathBuf::from("team-mcp")
}

pub fn agent_wrapper(root: &Path) -> PathBuf {
    root.join("bin/agent-wrapper.sh")
}
