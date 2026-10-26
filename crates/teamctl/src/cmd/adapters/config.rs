use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use team_core::compose::Compose;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub agent: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Destination environment key → operator environment variable name.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub allow_senders: Vec<String>,
    #[serde(default)]
    pub allow_peers: Vec<String>,
    #[serde(default = "options")]
    pub options: Value,
    #[serde(default = "timeout")]
    pub startup_timeout_secs: u64,
}
fn options() -> Value {
    serde_json::json!({})
}
fn timeout() -> u64 {
    90
}

#[derive(Clone)]
pub struct Spec {
    pub name: String,
    pub kind: String,
    pub config: Config,
}

pub fn specs(compose: &Compose) -> Result<Vec<Spec>> {
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for interface in &compose.global.interfaces {
        // Native Telegram continues through its existing per-manager setup.
        // Reserved legacy declarations without an executable remain inert.
        if !matches!(interface.r#type.as_str(), "whatsapp" | "process") {
            continue;
        }
        anyhow::ensure!(
            cfg!(unix),
            "process messaging adapters require a Unix host (macOS or Linux)"
        );
        anyhow::ensure!(
            !interface.name.is_empty()
                && interface.name.trim() == interface.name
                && interface.name.len() <= 128
                && !interface.name.chars().any(char::is_control),
            "adapter name must be nonempty, bounded and contain no control characters"
        );
        anyhow::ensure!(
            interface.name != "local" && !interface.name.starts_with("telegram:"),
            "adapter name is reserved for a built-in transport"
        );
        anyhow::ensure!(
            names.insert(&interface.name),
            "duplicate adapter instance name: {}",
            interface.name
        );
        let config: Config = serde_yaml::from_value(interface.config.clone())
            .with_context(|| format!("invalid adapter configuration for {}", interface.name))?;
        anyhow::ensure!(
            compose.agents().any(|h| h.id() == config.agent),
            "adapter {} references unknown canonical agent {}",
            interface.name,
            config.agent
        );
        anyhow::ensure!(
            (1..=300).contains(&config.startup_timeout_secs),
            "adapter startup timeout must be 1..300 seconds"
        );
        anyhow::ensure!(
            config.options.is_object(),
            "adapter options must be an object"
        );
        if interface.r#type != "whatsapp" && config.command.as_deref().is_none_or(str::is_empty) {
            bail!("adapter {} needs an executable command", interface.name);
        }
        for (key, source) in &config.env {
            let valid = |s: &str| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes().enumerate().all(|(i, c)| {
                        c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                    })
            };
            anyhow::ensure!(
                valid(key) && valid(source),
                "adapter env uses variable names, not literal values"
            );
        }
        anyhow::ensure!(
            config.args.iter().all(|s| !s.contains('\0')),
            "adapter args cannot contain NUL"
        );
        anyhow::ensure!(
            config.command.as_ref().is_none_or(|s| !s.contains('\0')),
            "adapter command cannot contain NUL"
        );
        for id in config.allow_peers.iter().chain(&config.allow_senders) {
            anyhow::ensure!(
                !id.is_empty()
                    && id.trim() == id
                    && id.len() <= 1024
                    && !id.chars().any(char::is_control),
                "adapter allowlist identity is invalid"
            );
        }
        out.push(Spec {
            name: interface.name.clone(),
            kind: interface.r#type.clone(),
            config,
        });
    }
    let mut sessions: BTreeSet<_> = compose
        .agents()
        .map(|h| {
            format!(
                "{}{}-{}",
                compose.global.supervisor.tmux_prefix, h.project, h.agent
            )
            .replace('.', "_")
        })
        .collect();
    sessions.extend(
        super::super::bot::bot_specs(compose)
            .into_iter()
            .map(|s| s.session.replace('.', "_")),
    );
    for spec in &out {
        anyhow::ensure!(
            sessions.insert(spec.session(compose).replace('.', "_")),
            "adapter session collides with another configured process"
        );
    }
    Ok(out)
}

pub fn find(compose: &Compose, name: &str) -> Result<Spec> {
    specs(compose)?
        .into_iter()
        .find(|s| s.name == name)
        .with_context(|| format!("unknown adapter: {name}"))
}

pub fn state_dir(root: &Path, name: &str) -> Result<PathBuf> {
    let base = if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        PathBuf::from(path)
    } else {
        PathBuf::from(
            std::env::var_os("HOME").context("adapter state requires HOME or XDG_STATE_HOME")?,
        )
        .join(".local/state")
    };
    anyhow::ensure!(
        base.is_absolute(),
        "adapter state base must be an absolute path"
    );
    let base = canonical_future(&base)?;
    let root = root.canonicalize()?;
    anyhow::ensure!(
        !base.starts_with(&root),
        "adapter state must be outside the team workspace; set XDG_STATE_HOME elsewhere"
    );
    let team = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    let instance = blake3::hash(name.as_bytes()).to_hex();
    Ok(base
        .join("teamctl/adapters")
        .join(&team[..24])
        .join(&instance[..24]))
}

pub fn private_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            private_dir(parent)?;
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "adapter private directory must not be a symlink"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(
                    meta.uid() == unsafe { libc::geteuid() },
                    "adapter private directory has a different owner"
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return private_dir(path)
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn canonical_future(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(Into::into);
    }
    let parent = path
        .parent()
        .context("adapter state path has no existing ancestor")?;
    let name = path.file_name().context("adapter state path is invalid")?;
    Ok(canonical_future(parent)?.join(name))
}

pub fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("open private adapter state")?;
    let meta = file.metadata()?;
    anyhow::ensure!(meta.is_file(), "adapter state must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            meta.uid() == unsafe { libc::geteuid() },
            "adapter state has a different owner"
        );
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// A lifetime lease that rejects a second host immediately and survives crashes.
pub struct HostLock {
    _connection: rusqlite::Connection,
}
impl HostLock {
    /// Inspect SQLite's reserved writer byte without creating files or taking a
    /// transaction. Foreground ownership cannot be inferred from a stale PID.
    pub fn owned(path: &Path) -> Result<bool> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "adapter ownership state is not a regular file"
        );
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = libc::F_WRLCK as _;
            lock.l_whence = libc::SEEK_SET as _;
            lock.l_start = 0x40000001;
            lock.l_len = 1;
            let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) };
            anyhow::ensure!(result != -1, "cannot inspect adapter lifetime ownership");
            Ok(i64::from(lock.l_type) != i64::from(libc::F_UNLCK))
        }
        #[cfg(not(unix))]
        {
            anyhow::bail!("read-only adapter ownership inspection is unsupported on this platform")
        }
    }
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::try_acquire(path)?.context("this adapter instance is already owned by another host")
    }
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        drop(private_file(path)?);
        let connection = rusqlite::Connection::open(path).context("open adapter ownership lock")?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        anyhow::ensure!(
            journal == "delete",
            "adapter ownership store must use the rollback journal"
        );
        match connection.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => Ok(Some(Self {
                _connection: connection,
            })),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error).context("acquire adapter ownership lock"),
        }
    }
}

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.len() <= limit,
        "private adapter file is invalid or exceeds its limit"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.permissions().mode() & 0o077 == 0,
            "private adapter file has unsafe ownership or permissions"
        );
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "private adapter file exceeds its limit"
    );
    Ok(bytes)
}

pub fn ensure_state_scope(compose: &Compose, state: &Path) -> Result<()> {
    anyhow::ensure!(state.is_absolute(), "adapter state must be absolute");
    let state = canonical_future(state)?;
    anyhow::ensure!(
        !state.starts_with(compose.root.canonicalize()?),
        "adapter state must be outside the team workspace"
    );
    for project in &compose.projects {
        let cwd = canonical_future(&compose.root.join(&project.project.cwd))?;
        anyhow::ensure!(
            !state.starts_with(cwd),
            "adapter state must be outside agent working directories"
        );
    }
    Ok(())
}

impl Spec {
    pub fn authorized(&self, peer: &str, sender: &str) -> bool {
        self.config.allow_peers.iter().any(|p| p == peer)
            && self.config.allow_senders.iter().any(|s| s == sender)
    }
    pub fn session(&self, compose: &Compose) -> String {
        let mut hash = blake3::Hasher::new();
        let root = compose
            .root
            .canonicalize()
            .unwrap_or_else(|_| compose.root.clone());
        hash.update(root.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(self.name.as_bytes());
        let suffix = hash.finalize().to_hex();
        format!(
            "{}adapter-{}",
            compose.global.supervisor.tmux_prefix,
            &suffix[..16]
        )
    }
    pub fn fingerprint(&self) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(serde_json::to_string(&self.config)?.as_bytes());
        hasher.update(self.kind.as_bytes());
        for source in self.config.env.values() {
            hasher.update(&serde_json::to_vec(&(source, std::env::var(source).ok()))?);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
}
