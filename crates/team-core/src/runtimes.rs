//! Runtime adapter descriptors.
//!
//! Canonical descriptors for the runtimes teamctl ships with (Claude Code,
//! Codex, Gemini) are baked into the binary via [`embedded_defaults`]. Users
//! can override or extend them by dropping their own `<root>/runtimes/<id>.yaml`
//! into the compose tree -- file-based descriptors win on key collision.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Canonical descriptors that ship with teamctl. Keep this list in sync
/// with the YAML files under `crates/team-core/runtimes/`.
const EMBEDDED: &[(&str, &str)] = &[
    ("claude-code", include_str!("../runtimes/claude-code.yaml")),
    ("codex", include_str!("../runtimes/codex.yaml")),
    ("gemini", include_str!("../runtimes/gemini.yaml")),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runtime {
    /// Path / name of the CLI binary (resolved on $PATH by the wrapper).
    pub binary: String,
    #[serde(default)]
    pub supports_mcp: bool,
    #[serde(default)]
    pub session_resume: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    /// Patterns that, if matched in the runtime's stdout/stderr, indicate a
    /// rate-limit hit. `teamctl rl-watch` consumes these.
    #[serde(default)]
    pub rate_limit_patterns: Vec<RateLimitPattern>,
}

/// One rate-limit detector. `match` is a regex tested against each line
/// of runtime output. If matched, the wrapper records a hit. The optional
/// captures attempt to extract when the limit lifts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitPattern {
    /// Regex tested against each output line.
    pub r#match: String,
    /// Optional regex with one capture group of an absolute reset clock,
    /// e.g. "resets at (4pm)" or "resets at (16:00)" or an RFC3339 timestamp.
    #[serde(default)]
    pub resets_at_capture: Option<String>,
    /// Optional regex with one capture group of a relative duration,
    /// e.g. "in (5h 15m)" or "in (1h)" or "(\\d+) seconds".
    #[serde(default)]
    pub resets_in_capture: Option<String>,
}

/// Embedded canonical runtime descriptors -- the ones teamctl ships with.
/// Always available; do not require any files on disk.
pub fn embedded_defaults() -> Result<BTreeMap<String, Runtime>> {
    EMBEDDED
        .iter()
        .map(|(stem, src)| {
            let r: Runtime = serde_yaml::from_str(src)
                .with_context(|| format!("parse embedded runtime `{stem}`"))?;
            Ok(((*stem).to_string(), r))
        })
        .collect()
}

/// Resolve the runtime adapter map for a compose tree.
///
/// Starts from the [`embedded_defaults`] (Claude Code / Codex / Gemini) and
/// overlays any `<root>/runtimes/<name>.yaml` files. File-based descriptors
/// override the embedded ones when keys collide and can introduce new
/// runtimes the binary has never heard of.
pub fn load_all(root: &Path) -> Result<BTreeMap<String, Runtime>> {
    let mut map = embedded_defaults()?;
    let dir = root.join("runtimes");
    if !dir.exists() {
        return Ok(map);
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path: PathBuf = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let content =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let r: Runtime =
            serde_yaml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
        map.insert(stem, r);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_parse() {
        let m = embedded_defaults().unwrap();
        assert!(m.contains_key("claude-code"));
        assert!(m.contains_key("codex"));
        assert!(m.contains_key("gemini"));
        assert_eq!(m["claude-code"].binary, "claude");
        assert!(m["claude-code"].supports_mcp);
    }

    #[test]
    fn load_nonexistent_returns_embedded_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let m = load_all(tmp.path()).unwrap();
        // No files on disk, but the embedded defaults must still be there.
        assert!(m.contains_key("claude-code"));
        assert!(m.contains_key("codex"));
        assert!(m.contains_key("gemini"));
    }

    #[test]
    fn user_file_overrides_embedded_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtimes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("claude-code.yaml"),
            "binary: my-claude-fork\nsupports_mcp: false\n",
        )
        .unwrap();
        let m = load_all(tmp.path()).unwrap();
        assert_eq!(m["claude-code"].binary, "my-claude-fork");
        assert!(!m["claude-code"].supports_mcp);
        // Other embedded defaults are untouched.
        assert_eq!(m["codex"].binary, "codex");
    }

    #[test]
    fn user_file_can_add_new_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtimes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("aider.yaml"),
            "binary: aider\nsupports_mcp: false\n",
        )
        .unwrap();
        let m = load_all(tmp.path()).unwrap();
        assert_eq!(m["aider"].binary, "aider");
        // Embedded defaults coexist.
        assert!(m.contains_key("claude-code"));
    }
}
