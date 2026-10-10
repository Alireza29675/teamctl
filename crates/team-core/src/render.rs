//! Render a loaded compose into on-disk artifacts.
//!
//! Outputs under `<root>/state/`:
//! - `envs/<project>-<agent>.env`      — env vars for the agent wrapper.
//! - `mcp/<project>-<agent>.json`      — MCP stdio config for the runtime.
//! - `claude/<project>-<agent>.json`   — wrapper-managed Claude Code
//!   settings (currently a `PreToolUse` deny hook for synchronous-prompt
//!   tools that strand a headless pane). Claude-code agents only.
//! - `codex-home/<project>-<agent>/config.toml` — per-agent Codex home
//!   carrying the same MCP servers as the JSON above in the
//!   `[mcp_servers.<name>]` table form codex reads (codex has no
//!   `--mcp-config` flag). Codex agents only.
//! - `opencode-home/<project>-<agent>/opencode.json` — per-agent OpenCode
//!   config carrying the same MCP servers in the `mcp` object form
//!   opencode reads via `OPENCODE_CONFIG` (opencode has no `--mcp-config`
//!   flag either), plus `instructions` / autoupdate + share opt-outs.
//!   The agent's session db lives beside it (`OPENCODE_DB`). OpenCode
//!   agents only.
//! - `role_prompts/<project>-<agent>.md` (multi-file role_prompt only) —
//!   the ordered concatenation of every source file declared in the
//!   role's `role_prompt: [...]` list. Re-materialized on every render
//!   so any source-file edit lands in the agent's prompt at next boot.
//!
//! `systemd` / `launchd` unit rendering lives behind a feature flag when
//! those back-ends are enabled via `supervisor.type`.

use std::io;
use std::path::{Path, PathBuf};

use crate::compose::{AgentHandle, Compose, RolePrompt};

/// Separator written between concatenated role-prompt files. Em-dash
/// framed by blank lines reads cleanly when an operator inspects the
/// materialized file under `state/role_prompts/`.
const ROLE_PROMPT_SEPARATOR: &str = "\n\n—\n\n";

mod claude;
mod codex;
mod env;
mod extensions;
mod mcp;
mod opencode;
mod paths;

pub use claude::render_claude_settings;
#[cfg(test)]
use codex::CODEX_PREAPPROVED_TEAM_TOOLS;
pub use codex::{render_codex_config, write_codex_config};
use env::render_env;
pub use env::{system_prompt_path, write_role_prompt_concat, write_runtime_launcher};
pub use extensions::{render_subagents, write_agent_skills, write_subagents_json};
use mcp::{render_mcp, runtime_supports_mcp, team_server_args};
pub use opencode::{render_opencode_config, write_opencode_config};
pub use paths::*;

/// Rendered env + MCP content for a single agent.
pub fn render_agent(
    compose: &Compose,
    handle: AgentHandle<'_>,
    team_mcp_bin: &str,
) -> (String, String) {
    let env = render_env(compose, handle);
    let mcp = render_mcp(compose, handle, team_mcp_bin);
    (env, mcp)
}

#[cfg(test)]
mod tests;
