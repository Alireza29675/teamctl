//! Non-evaluating dotenv references for managed helpers. Values never enter argv.
use anyhow::{bail, Context, Result};
use std::{collections::BTreeMap, path::Path};

pub fn parse_dotenv(raw: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !crate::mcp::valid_env_name(key) {
            continue;
        }
        let value = value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        values.insert(key.into(), value.into());
    }
    values
}

/// Team dotenv takes precedence over inherited state, including stale tmux
/// server environments. An explicit empty assignment does not resurrect a stale key.
pub fn lookup(root: Option<&Path>, name: &str) -> Result<Option<String>> {
    lookup_with(root, name, |name| std::env::var(name).ok())
}

pub fn lookup_with(
    root: Option<&Path>,
    name: &str,
    inherited: impl FnOnce(&str) -> Option<String>,
) -> Result<Option<String>> {
    if !crate::mcp::valid_env_name(name) {
        bail!("invalid environment variable reference");
    }
    if let Some(root) = root {
        for path in [
            root.join(".env"),
            root.parent().unwrap_or(root).join(".env"),
        ] {
            match std::fs::read_to_string(&path) {
                Ok(raw) => {
                    if let Some(value) = parse_dotenv(&raw).remove(name) {
                        return Ok(Some(value));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("read environment file {}", path.display()))
                }
            }
        }
    }
    Ok(inherited(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dotenv_references_are_literal_and_override_stale_inherited_values() {
        let root = tempfile::tempdir().unwrap();
        let team = root.path().join(".team");
        std::fs::create_dir(&team).unwrap();
        std::fs::write(
            root.path().join(".env"),
            "TOKEN=parent\nPARENT_ONLY=parent\n",
        )
        .unwrap();
        std::fs::write(
            team.join(".env"),
            "# local\nexport TOKEN='rotated-key'\nEMPTY=\nLITERAL='$(do-not-execute)'\n",
        )
        .unwrap();
        assert_eq!(
            lookup_with(Some(&team), "TOKEN", |_| Some("stale-key".into()))
                .unwrap()
                .as_deref(),
            Some("rotated-key")
        );
        assert_eq!(
            lookup_with(Some(&team), "EMPTY", |_| Some("stale-key".into()))
                .unwrap()
                .as_deref(),
            Some("")
        );
        assert_eq!(
            lookup_with(Some(&team), "LITERAL", |_| None)
                .unwrap()
                .as_deref(),
            Some("$(do-not-execute)")
        );
        assert_eq!(
            lookup_with(Some(&team), "PARENT_ONLY", |_| None)
                .unwrap()
                .as_deref(),
            Some("parent")
        );
        assert_eq!(
            lookup_with(Some(&team), "HOST_ONLY", |_| Some("host".into()))
                .unwrap()
                .as_deref(),
            Some("host")
        );
        assert!(lookup_with(Some(&team), "TOKEN;echo", |_| None).is_err());
    }
}
