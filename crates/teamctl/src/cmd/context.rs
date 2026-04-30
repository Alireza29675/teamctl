//! `teamctl context` — name multiple `.team/` roots so you can jump
//! between them without typing `-C <path>` every time.
//!
//! **Deprecated in 0.4.0.** T-008 retired registered-context root
//! resolution: `teamctl` now finds `.team/` only via walk-up from cwd or
//! an explicit `-C <path>` / `TEAMCTL_ROOT`. The subcommands here remain
//! as a stub for one release so existing scripts don't break, but every
//! invocation prints a deprecation note to stderr and the registered
//! context is no longer consulted by root resolution.
//!
//! State lives at `~/.config/teamctl/contexts.json`:
//!
//! ```json
//! {
//!   "current": "newsroom",
//!   "contexts": {
//!     "newsroom": "/Users/you/dev/projects/news/.team",
//!     "startup":  "/Users/you/dev/projects/startup/.team"
//!   }
//! }
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct ContextStore {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub contexts: BTreeMap<String, PathBuf>,
}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".config/teamctl/contexts.json"))
}

pub fn load() -> Result<ContextStore> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Default::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn save(store: &ContextStore) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(store)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Print the one-line deprecation notice. Called from every public
/// subcommand entry point so an operator running any `teamctl context …`
/// invocation sees the upcoming-removal note.
fn deprecation_notice() {
    eprintln!(
        "warning: `teamctl context` is deprecated and will be removed in 0.4.x. \
         teamctl now resolves `.team/` via walk-up from cwd or `-C <path>` only."
    );
}

/// Resolve the active context to its root path and name. Retained as a
/// stub for the deprecation window; no longer consulted by CLI root
/// resolution after T-008.
#[allow(dead_code)]
pub fn root_for_current_named() -> Result<Option<(String, PathBuf)>> {
    let store = load()?;
    let Some(name) = store.current else {
        return Ok(None);
    };
    Ok(store.contexts.get(&name).cloned().map(|p| (name, p)))
}

/// Auto-register a root when `teamctl up` runs against it. Idempotent.
/// Retained as a stub for the deprecation window; no longer invoked
/// after T-008 (the `Up` command path stopped calling it).
#[allow(dead_code)]
pub fn auto_register(root: &Path) -> Result<()> {
    let mut store = load()?;
    let abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Default name = parent dir basename, or .team/ parent if root itself
    // is the .team folder.
    let name_source = if abs.file_name().map(|s| s == ".team").unwrap_or(false) {
        abs.parent().unwrap_or(&abs)
    } else {
        &abs
    };
    let mut name = name_source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("default")
        .to_string();
    // Don't overwrite an existing entry that points elsewhere.
    if let Some(existing) = store.contexts.get(&name) {
        if existing != &abs {
            // Disambiguate.
            let mut n = 1;
            loop {
                let candidate = format!("{name}-{n}");
                if !store.contexts.contains_key(&candidate) {
                    name = candidate;
                    break;
                }
                n += 1;
            }
        } else {
            return Ok(()); // already registered with the same path
        }
    }
    store.contexts.insert(name.clone(), abs);
    if store.current.is_none() {
        store.current = Some(name);
    }
    save(&store)
}

pub fn ls() -> Result<()> {
    deprecation_notice();
    let store = load()?;
    if store.contexts.is_empty() {
        println!("(no contexts registered yet — `teamctl up` auto-registers one)");
        return Ok(());
    }
    println!("{:<3} {:<20} PATH", "*", "NAME");
    for (name, path) in &store.contexts {
        let mark = if store.current.as_deref() == Some(name.as_str()) {
            "*"
        } else {
            " "
        };
        println!("{mark:<3} {name:<20} {}", path.display());
    }
    Ok(())
}

pub fn current() -> Result<()> {
    deprecation_notice();
    let store = load()?;
    match store.current {
        Some(n) => println!("{n}"),
        None => println!("(none)"),
    }
    Ok(())
}

pub fn use_(name: &str) -> Result<()> {
    deprecation_notice();
    let mut store = load()?;
    if !store.contexts.contains_key(name) {
        bail!("no context named `{name}`. `teamctl context ls` to see options.");
    }
    store.current = Some(name.into());
    save(&store)?;
    println!("now using context `{name}`");
    Ok(())
}

pub fn add(name: &str, path: &Path) -> Result<()> {
    deprecation_notice();
    let abs = path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))?;
    if !abs.join("team-compose.yaml").is_file() {
        bail!(
            "{} does not contain a team-compose.yaml — pass the directory holding it (e.g. `…/.team`)",
            abs.display()
        );
    }
    let mut store = load()?;
    store.contexts.insert(name.into(), abs.clone());
    if store.current.is_none() {
        store.current = Some(name.into());
    }
    save(&store)?;
    println!("added context `{name}` → {}", abs.display());
    Ok(())
}

pub fn rm(name: &str) -> Result<()> {
    deprecation_notice();
    let mut store = load()?;
    if store.contexts.remove(name).is_none() {
        bail!("no context named `{name}`");
    }
    if store.current.as_deref() == Some(name) {
        store.current = None;
    }
    save(&store)?;
    println!("removed context `{name}`");
    Ok(())
}
