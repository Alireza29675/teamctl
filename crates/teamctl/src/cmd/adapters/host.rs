//! Private carrier files and lifetime ownership for managed/foreground hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use team_core::compose::Compose;

use super::config::{self, HostLock, Spec};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carrier {
    root: PathBuf,
    instance: String,
    pub state_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

pub fn prepare(compose: &Compose, spec: &Spec, state: &Path) -> Result<PathBuf> {
    config::ensure_state_scope(compose, state)?;
    config::private_dir(state)?;
    let mut env = BTreeMap::new();
    for source in spec.config.env.values() {
        env.insert(
            source.clone(),
            std::env::var(source).map_err(|_| {
                anyhow::anyhow!("adapter credential environment {source} is unset or not UTF-8")
            })?,
        );
    }
    let carrier = Carrier {
        root: compose.root.canonicalize()?,
        instance: spec.name.clone(),
        state_dir: state.canonicalize()?,
        env,
    };
    let path = state.join("host-context.json");
    team_core::persistence::atomic_write(&path, &serde_json::to_vec(&carrier)?)?;
    Ok(path)
}

pub fn read(path: &Path, compose: &Compose, spec: &Spec) -> Result<Carrier> {
    let carrier: Carrier = serde_json::from_slice(&config::read_private(path, 1024 * 1024)?)
        .map_err(|_| anyhow::anyhow!("invalid private adapter context"))?;
    anyhow::ensure!(
        carrier.root == compose.root.canonicalize()? && carrier.instance == spec.name,
        "private adapter context belongs to another team or instance"
    );
    config::ensure_state_scope(compose, &carrier.state_dir)?;
    let parent = path
        .parent()
        .context("adapter context has no directory")?
        .canonicalize()?;
    anyhow::ensure!(
        parent == carrier.state_dir.canonicalize()?,
        "adapter context state directory mismatch"
    );
    let expected: BTreeSet<_> = spec.config.env.values().cloned().collect();
    anyhow::ensure!(
        carrier.env.keys().cloned().collect::<BTreeSet<_>>() == expected,
        "adapter environment mapping changed; restart its managed host"
    );
    Ok(carrier)
}

fn directory(compose: &Compose, name: &str) -> Result<PathBuf> {
    let base = team_core::registry::config_dir().context("adapter host ownership requires HOME")?;
    let root = compose.root.canonicalize()?;
    let team = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    let instance = blake3::hash(name.as_bytes()).to_hex();
    let directory = base
        .join("adapter-hosts")
        .join(&team[..24])
        .join(&instance[..24]);
    anyhow::ensure!(
        !directory.starts_with(compose.root.canonicalize()?),
        "adapter host ownership must be outside the team workspace"
    );
    config::private_dir(&directory)?;
    Ok(directory)
}

#[derive(Serialize, Deserialize)]
struct Control {
    nonce: String,
    state_dir: PathBuf,
    #[serde(default)]
    name: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    pid: u32,
}

pub struct Live {
    pub name: String,
    pub agent: String,
}

pub struct Lease {
    directory: PathBuf,
    nonce: String,
    _identity: HostLock,
    _profile: HostLock,
}

impl Lease {
    pub fn acquire(compose: &Compose, name: &str, agent: &str, state: &Path) -> Result<Self> {
        let directory = directory(compose, name)?;
        config::ensure_state_scope(compose, &directory)?;
        let identity = HostLock::acquire(&directory.join("owner.sqlite")).context(
            "adapter profile is in use; stop this instance before pairing or inspection",
        )?;
        let profile = HostLock::acquire(&state.join("host.lock.sqlite"))?;
        let nonce = super::super::chat::request_id();
        let control = Control {
            nonce: nonce.clone(),
            state_dir: state.to_path_buf(),
            name: name.to_owned(),
            agent: agent.to_owned(),
            pid: std::process::id(),
        };
        team_core::persistence::atomic_write(
            &directory.join("control.json"),
            &serde_json::to_vec(&control)?,
        )?;
        Ok(Self {
            directory,
            nonce,
            _identity: identity,
            _profile: profile,
        })
    }

    pub fn stop_requested(&self) -> Result<bool> {
        match config::read_private(&self.directory.join("stop.request"), 1024) {
            Ok(bytes) => Ok(bytes == self.nonce.as_bytes()),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

/// Read-only inventory also retains the original target after config removal.
pub fn live(compose: &Compose) -> Result<Vec<Live>> {
    let Some(base) = team_core::registry::config_dir() else {
        return Ok(Vec::new());
    };
    let root = compose.root.canonicalize()?;
    let team = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    let parent = base.join("adapter-hosts").join(&team[..24]);
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut active = Vec::new();
    for (index, entry) in entries.enumerate() {
        anyhow::ensure!(index < 4096, "adapter host inventory exceeds its limit");
        let entry = entry?;
        anyhow::ensure!(
            entry.file_type()?.is_dir(),
            "adapter host inventory contains an unexpected entry"
        );
        if !HostLock::owned(&entry.path().join("owner.sqlite"))? {
            continue;
        }
        let bytes = match config::read_private(&entry.path().join("control.json"), 16 * 1024) {
            Ok(bytes) => bytes,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue
            }
            Err(error) => return Err(error),
        };
        let control: Control = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid adapter host control state"))?;
        if control.pid == 0 || control.name.is_empty() {
            continue;
        }
        anyhow::ensure!(
            control.pid <= i32::MAX as u32
                && control.name.len() <= 128
                && !control.name.chars().any(char::is_control)
                && control.agent.len() <= 1024
                && control.agent.split_once(':').is_some()
                && !control.agent.chars().any(char::is_control),
            "invalid adapter host inventory identity"
        );
        let hash = blake3::hash(control.name.as_bytes()).to_hex();
        anyhow::ensure!(
            entry.file_name() == std::ffi::OsStr::new(&hash[..24]),
            "adapter host inventory identity mismatch"
        );
        active.push(Live {
            name: control.name,
            agent: control.agent,
        });
    }
    Ok(active)
}

impl Drop for Lease {
    fn drop(&mut self) {
        // The lifetime lock is still held while removing only our control token.
        let _ = std::fs::remove_file(self.directory.join("control.json"));
        let _ = std::fs::remove_file(self.directory.join("stop.request"));
    }
}

pub fn request_stop(compose: &Compose, name: &str) -> Result<bool> {
    let directory = directory(compose, name)?;
    let control: Control = match config::read_private(&directory.join("control.json"), 16 * 1024) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid adapter host control state"))?,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    if HostLock::try_acquire(&directory.join("owner.sqlite"))?.is_some() {
        return Ok(false);
    }
    anyhow::ensure!(
        !control.nonce.is_empty() && control.nonce.len() <= 1024,
        "invalid adapter host control token"
    );
    team_core::persistence::atomic_write(
        &directory.join("stop.request"),
        control.nonce.as_bytes(),
    )?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if HostLock::try_acquire(&directory.join("owner.sqlite"))?.is_some() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("adapter host did not stop within its cleanup budget; ownership is retained")
}

pub fn ensure_available(compose: &Compose, name: &str) -> Result<()> {
    let directory = directory(compose, name)?;
    anyhow::ensure!(
        HostLock::try_acquire(&directory.join("owner.sqlite"))?.is_some(),
        "adapter profile is in use; stop this instance before starting another host"
    );
    Ok(())
}
