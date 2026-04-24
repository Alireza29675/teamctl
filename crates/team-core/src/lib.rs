//! Shared library for teamctl.
//!
//! This crate is the single place where YAML schema types, validators, and
//! artifact renderers live. The three binaries (`teamctl`, `team-mcp`,
//! `team-bot`) depend on it but never on each other.
//!
//! Phase 0 intentionally exposes only version constants so the workspace
//! compiles. Phase 1 adds the `compose` and `render` modules.

/// Semantic version of the teamctl workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Minimum supported MCP protocol version. Matches Claude Code, Codex CLI,
/// and Gemini CLI at the time of writing.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn mcp_version_pinned() {
        assert_eq!(MCP_PROTOCOL_VERSION, "2024-11-05");
    }
}
