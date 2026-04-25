//! `teamctl env [--doctor]`: list every env var compose references and
//! whether it is set; with `--doctor`, fail if anything required is unset.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};

pub fn run(root: &Path, doctor: bool) -> Result<()> {
    // Auto-source `.team/.env` (or `<root>/.env`) so the user doesn't have
    // to remember to do it.
    let env_files = [
        root.join(".env"),
        root.parent().unwrap_or(root).join(".env"),
    ];
    for f in &env_files {
        if f.is_file() {
            source_dotenv(f);
        }
    }

    let compose = super::load(root)?;
    let mut required: BTreeMap<String, String> = BTreeMap::new();

    // 1. Interfaces — `*_env` fields in the YAML config.
    for iface in &compose.global.interfaces {
        if let serde_yaml::Value::Mapping(m) = &iface.config {
            for (k, v) in m {
                if let (Some(key), Some(val)) = (k.as_str(), v.as_str()) {
                    if key.ends_with("_env") {
                        required.insert(
                            val.to_string(),
                            format!("interfaces[{}].config.{}", iface.name, key),
                        );
                    }
                }
            }
        }
    }

    // 2. Rate-limit hooks of `webhook` action with `url_env`.
    for hook in &compose.global.rate_limits.hooks {
        if let Some(env) = &hook.url_env {
            required.insert(
                env.clone(),
                format!("rate_limits.hooks[{}].url_env", hook.name),
            );
        }
    }

    if required.is_empty() {
        println!("(compose tree references no env vars)");
        return Ok(());
    }

    let mut missing = 0;
    println!("{:<32} {:<8} REFERENCED FROM", "VAR", "STATE");
    for (var, refd) in &required {
        let val = std::env::var(var).ok();
        let state = match &val {
            Some(v) if !v.is_empty() => format!("set ({})", mask(v)),
            _ => {
                missing += 1;
                "UNSET".into()
            }
        };
        println!("{var:<32} {state:<8} {refd}");
    }

    if doctor && missing > 0 {
        bail!("{missing} required env var(s) unset");
    }
    Ok(())
}

fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n <= 4 {
        return "****".into();
    }
    let last4: String = s
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("****{last4}")
}

fn source_dotenv(path: &Path) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            // Strip optional quotes
            let v = v.trim();
            let v = v.trim_matches('"').trim_matches('\'');
            // Only set if not already set in the environment.
            if std::env::var_os(k).is_none() {
                // SAFETY: setting env vars in single-threaded CLI startup is fine.
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}
