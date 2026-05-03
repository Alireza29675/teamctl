//! Shared library for teamctl.
//!
//! Exposes the YAML schema types for the compose tree, the validator that
//! enforces project-isolation + ACL invariants, the artifact renderer that
//! turns compose into env files and MCP configs, the `Supervisor` trait
//! (with a portable `TmuxSupervisor` back-end), and the SQLite mailbox
//! schema used by `team-mcp`.

pub mod compose;
pub mod mailbox;
pub mod render;
pub mod runtimes;
pub mod supervisor;
pub mod validate;
pub mod worktree;
pub mod yaml_edit;

/// Semantic version of the teamctl workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP protocol version teamctl speaks. Pinned per release.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
