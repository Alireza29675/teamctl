use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};
use std::process::Command;

// Never embed node_modules, profiles or caches in the executable.
static SDK: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../adapters/sdk/src");
static WHATSAPP: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../adapters/whatsapp/src");
const MANIFESTS: [(&str, &str); 4] = [
    (
        "package.json",
        include_str!("../../../../../adapters/package.json"),
    ),
    (
        "package-lock.json",
        include_str!("../../../../../adapters/package-lock.json"),
    ),
    (
        "sdk/package.json",
        include_str!("../../../../../adapters/sdk/package.json"),
    ),
    (
        "whatsapp/package.json",
        include_str!("../../../../../adapters/whatsapp/package.json"),
    ),
];
const MARKER: &str = "installed.lock-hash";

fn lock_hash() -> String {
    let mut hash = blake3::Hasher::new();
    for (path, contents) in MANIFESTS {
        hash.update(path.as_bytes());
        hash.update(contents.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}
fn hash_source(hash: &mut blake3::Hasher, directory: &Dir<'_>) {
    for file in directory.files() {
        hash.update(file.path().as_os_str().as_encoded_bytes());
        hash.update(file.contents());
    }
    for child in directory.dirs() {
        hash_source(hash, child);
    }
}
pub fn fingerprint() -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(lock_hash().as_bytes());
    hash_source(&mut hash, &SDK);
    hash_source(&mut hash, &WHATSAPP);
    hash.finalize().to_hex().to_string()
}

pub fn root(team: &Path) -> Result<PathBuf> {
    Ok(super::config::state_dir(team, "builtin-assets")?.join("node"))
}

pub fn materialize(team: &Path) -> Result<PathBuf> {
    let dest = root(team)?;
    super::config::private_dir(&dest)?;
    for (relative, text) in MANIFESTS {
        team_core::persistence::atomic_write(&dest.join(relative), text.as_bytes())?;
    }
    SDK.extract(dest.join("sdk/src"))?;
    WHATSAPP.extract(dest.join("whatsapp/src"))?;
    Ok(dest)
}

pub fn install(team: &Path) -> Result<()> {
    let dest = root(team)?;
    super::config::private_dir(&dest)?;
    let _owner = super::config::HostLock::acquire(&dest.join("install.lock.sqlite"))?;
    let dest = materialize(team)?;
    let version = Command::new("node")
        .args(["-p", "process.versions.node"])
        .output()
        .context("WhatsApp adapter requires Node22.12 or newer")?;
    anyhow::ensure!(
        version.status.success() && supported_node(String::from_utf8_lossy(&version.stdout).trim()),
        "WhatsApp adapter requires Node22.12 or newer"
    );
    match std::fs::remove_file(dest.join(MARKER)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let status = Command::new("npm")
        .args(["ci", "--omit=dev", "--no-fund", "--no-audit"])
        .current_dir(&dest)
        .status()
        .context("install pinned adapter dependencies")?;
    anyhow::ensure!(status.success(), "adapter dependency installation failed");
    team_core::persistence::atomic_write(&dest.join(MARKER), lock_hash().as_bytes())?;
    println!("installed · WhatsApp adapter · {}", dest.display());
    Ok(())
}

pub fn command(team: &Path) -> Result<(String, Vec<String>)> {
    let dest = root(team)?;
    super::config::private_dir(&dest)?;
    let _owner = super::config::HostLock::acquire(&dest.join("install.lock.sqlite"))?;
    anyhow::ensure!(
        installed(&dest)?,
        "WhatsApp dependencies do not match this build; run `teamctl adapter setup <name>` first"
    );
    materialize(team)?;
    Ok((
        "node".into(),
        vec![dest
            .join("whatsapp/src/main.js")
            .to_string_lossy()
            .into_owned()],
    ))
}

fn supported_node(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').map(str::parse::<u32>).collect();
    matches!(parts.as_slice(), [Ok(major), Ok(minor), Ok(_)] if *major>22 || (*major==22 && *minor>=12))
}
fn installed(dest: &Path) -> Result<bool> {
    let marker = match super::config::read_private(&dest.join(MARKER), 1024) {
        Ok(value) => value,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(false)
        }
        Err(e) => return Err(e),
    };
    Ok(marker == lock_hash().as_bytes()
        && dest
            .join("node_modules/whatsapp-web.js/package.json")
            .is_file()
        && dest.join("node_modules/puppeteer/package.json").is_file())
}

pub fn available(team: &Path) -> Result<bool> {
    installed(&root(team)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn node_floor_and_malformed_versions() {
        for version in ["22.12.0", "22.23.1", "24.0.0"] {
            assert!(supported_node(version));
        }
        for version in ["22.11.0", "20.19.0", "22.12", "22.12.bad", "bad"] {
            assert!(!supported_node(version));
        }
    }
    #[test]
    fn installation_requires_matching_embedded_lock_marker() {
        let temp = tempfile::tempdir().unwrap();
        for package in ["whatsapp-web.js", "puppeteer"] {
            let path = temp.path().join(format!("node_modules/{package}"));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("package.json"), "{}").unwrap();
        }
        assert!(!installed(temp.path()).unwrap());
        std::fs::write(temp.path().join(MARKER), "old-lock").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                temp.path().join(MARKER),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        assert!(!installed(temp.path()).unwrap());
        std::fs::write(temp.path().join(MARKER), lock_hash()).unwrap();
        assert!(installed(temp.path()).unwrap());
    }
}
