//! Runtime adapter descriptors (`runtimes/*.yaml`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
}

/// Load every `runtimes/<name>.yaml` under the compose root into a map keyed
/// by the file stem (so `claude-code.yaml` → key `"claude-code"`).
pub fn load_all(root: &Path) -> Result<BTreeMap<String, Runtime>> {
    let dir = root.join("runtimes");
    let mut map = BTreeMap::new();
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
    fn load_nonexistent_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let m = load_all(tmp.path()).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn load_parses_runtimes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtimes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("claude-code.yaml"),
            "binary: claude\nsupports_mcp: true\ndefault_model: claude-opus-4-7\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("codex.yaml"),
            "binary: codex\nsupports_mcp: true\n",
        )
        .unwrap();
        let m = load_all(tmp.path()).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m["claude-code"].binary, "claude");
        assert!(m["claude-code"].supports_mcp);
        assert_eq!(m["codex"].binary, "codex");
    }
}
