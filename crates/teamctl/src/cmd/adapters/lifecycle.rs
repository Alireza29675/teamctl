//! Persist exact adapter identities so prefix/config removal remains recoverable.

use super::config::Spec;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

#[derive(Clone, Serialize, Deserialize)]
pub struct Applied {
    pub session: String,
    pub agent: String,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<PathBuf>,
}
pub fn load(root: &Path) -> Result<BTreeMap<String, Applied>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(root.join("state/adapters.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    const LIMIT: u64 = 4 * 1024 * 1024;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= LIMIT,
        "persisted adapter identities are invalid or exceed their limit"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= LIMIT,
        "persisted adapter identities exceed their limit"
    );
    let identities: BTreeMap<String, Applied> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid persisted adapter identities"))?;
    for (name, identity) in &identities {
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 128
                && !name.chars().any(char::is_control)
                && !identity.session.is_empty()
                && identity.session.len() <= 1024
                && !identity.session.chars().any(char::is_control)
                && identity.agent.split_once(':').is_some()
                && !identity.agent.chars().any(char::is_control),
            "invalid persisted adapter identity"
        );
    }
    Ok(identities)
}
fn save(root: &Path, applied: &BTreeMap<String, Applied>) -> Result<()> {
    team_core::persistence::atomic_write(
        &root.join("state/adapters.json"),
        &serde_json::to_vec_pretty(applied)?,
    )?;
    Ok(())
}
fn agent_spec(compose: &Compose, name: &str, session: &str) -> AgentSpec {
    AgentSpec {
        project: "adapter".into(),
        agent: name.into(),
        tmux_session: session.into(),
        wrapper: PathBuf::new(),
        cwd: compose.root.clone(),
        env_file: PathBuf::new(),
    }
}
pub fn observed(compose: &Compose, spec: &Spec) -> Result<AgentState> {
    if super::host::live(compose)?
        .iter()
        .any(|live| live.name == spec.name && live.agent == spec.config.agent)
    {
        return Ok(AgentState::Running);
    }
    let saved = load(&compose.root)?;
    let session = saved
        .get(&spec.name)
        .map(|row| row.session.clone())
        .unwrap_or_else(|| spec.session(compose));
    TmuxSupervisor.state(&agent_spec(compose, &spec.name, &session))
}
fn selected(agent: &str, project: Option<&str>, targets: Option<&BTreeSet<String>>) -> bool {
    let Some((p, a)) = agent.split_once(':') else {
        return false;
    };
    project.is_none_or(|scope| scope == p) && targets.is_none_or(|set| set.contains(a))
}
pub fn session_names(compose: &Compose, project: Option<&str>) -> Result<BTreeSet<String>> {
    // Persisted identities remain available for teardown after a removed agent
    // makes the desired adapter declaration invalid.
    let mut names: BTreeSet<_> = super::config::specs(compose)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| selected(&s.config.agent, project, None))
        .map(|s| s.session(compose))
        .collect();
    names.extend(
        load(&compose.root)?
            .into_values()
            .filter(|s| selected(&s.agent, project, None))
            .map(|s| s.session),
    );
    Ok(names)
}
pub fn down(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
) -> Result<()> {
    let mut applied = load(&compose.root)?;
    let mut stopped = BTreeSet::new();
    for (name, old) in applied.clone() {
        if !selected(&old.agent, project, targets) {
            continue;
        }
        stop(compose, &name, &old.session)?;
        applied.remove(&name);
        save(&compose.root, &applied)?;
        println!("stopped · adapter {name}");
        stopped.insert(name);
    }
    for live in super::host::live(compose)? {
        if selected(&live.agent, project, targets) && !stopped.contains(&live.name) {
            super::host::request_stop(compose, &live.name)?;
            println!("stopped · adapter {}", live.name);
        }
    }
    Ok(())
}
pub fn stop_one(compose: &Compose, name: &str) -> Result<()> {
    let mut applied = load(&compose.root)?;
    if let Some(old) = applied.get(name) {
        stop(compose, name, &old.session)?;
        applied.remove(name);
        save(&compose.root, &applied)?;
    } else {
        super::host::request_stop(compose, name)?;
    }
    println!("stopped · adapter {name}");
    Ok(())
}
pub fn start_one(compose: &Compose, name: &str) -> Result<()> {
    super::config::find(compose, name)?;
    preflight_selected(compose, None, None, Some(name))?;
    reconcile_selected(compose, None, None, false, false, Some(name))
}
pub fn preflight(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
) -> Result<()> {
    preflight_selected(compose, project, targets, None)
}
fn preflight_selected(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    only: Option<&str>,
) -> Result<()> {
    validate_scope_selected(compose, project, targets, only)?;
    for spec in super::config::specs(compose)? {
        if !selected(&spec.config.agent, project, targets)
            || only.is_some_and(|name| name != spec.name)
        {
            continue;
        }
        let state = super::config::state_dir(&compose.root, &spec.name)?;
        super::config::ensure_state_scope(compose, &state)?;
        anyhow::ensure!(
            spec.config
                .env
                .values()
                .all(|source| std::env::var(source).is_ok()),
            "adapter {} has an unset or non-UTF-8 credential environment reference",
            spec.name
        );
        if spec.kind == "whatsapp" {
            anyhow::ensure!(super::assets::available(&compose.root)?,"WhatsApp adapter {} needs matching dependencies; run `teamctl adapter setup {}` first",spec.name,spec.name);
        }
    }
    Ok(())
}

pub fn validate_scope(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
) -> Result<()> {
    validate_scope_selected(compose, project, targets, None)
}
fn validate_scope_selected(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    only: Option<&str>,
) -> Result<()> {
    let desired: BTreeMap<_, _> = super::config::specs(compose)?
        .into_iter()
        .map(|spec| (spec.name.clone(), spec))
        .collect();
    let mut owned: Vec<_> = load(&compose.root)?
        .into_iter()
        .map(|(name, row)| (name, row.agent))
        .collect();
    owned.extend(
        super::host::live(compose)?
            .into_iter()
            .map(|live| (live.name, live.agent)),
    );
    for (name, agent) in owned {
        if only.is_some_and(|selected| selected != name) {
            continue;
        }
        if let Some(spec) = desired.get(&name) {
            anyhow::ensure!(!(agent!=spec.config.agent&&selected(&spec.config.agent,project,targets)&&!selected(&agent,project,targets)),"adapter routing changed across the selected scope; reload both its old and new targets together");
        }
    }
    Ok(())
}
pub fn reconcile(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    dry: bool,
    force: bool,
) -> Result<()> {
    reconcile_selected(compose, project, targets, dry, force, None)
}
fn reconcile_selected(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    dry: bool,
    force: bool,
    only: Option<&str>,
) -> Result<()> {
    validate_scope_selected(compose, project, targets, only)?;
    let desired: BTreeMap<_, _> = super::config::specs(compose)?
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    let mut applied = load(&compose.root)?;
    for (name, old) in &applied {
        if only.is_some_and(|selected| selected != name) {
            continue;
        }
        if let Some(spec) = desired.get(name) {
            anyhow::ensure!(!(old.agent!=spec.config.agent && selected(&spec.config.agent,project,targets) && !selected(&old.agent,project,targets)), "adapter routing changed across the selected scope; reload both its old and new targets together");
        }
    }
    for (name, old) in applied.clone() {
        if only.is_some_and(|selected| selected != name) {
            continue;
        }
        if !selected(&old.agent, project, targets) {
            continue;
        }
        let changed = desired
            .get(&name)
            .map(|s| fingerprint(compose, s))
            .transpose()?
            .is_none_or(|hash| hash != old.fingerprint);
        if changed || force {
            if dry {
                println!("would stop · adapter {name}");
                continue;
            }
            stop(compose, &name, &old.session)?;
            applied.remove(&name);
            save(&compose.root, &applied)?;
        }
    }
    for (name, spec) in desired {
        if only.is_some_and(|selected| selected != name) {
            continue;
        }
        if !selected(&spec.config.agent, project, targets) {
            continue;
        }
        let session = spec.session(compose);
        let old = applied.get(&name);
        let current = old.map(|s| s.session.as_str()).unwrap_or(&session);
        let running =
            TmuxSupervisor.state(&agent_spec(compose, &name, current))? == AgentState::Running;
        let desired_fingerprint = fingerprint(compose, &spec)?;
        let changed = old.is_none_or(|s| desired_fingerprint != s.fingerprint);
        if running && !changed && !force {
            continue;
        }
        anyhow::ensure!(
            !running || old.is_some(),
            "adapter tmux identity is already occupied without this root's applied ownership"
        );
        if dry {
            println!("would start · adapter {name}");
            continue;
        }
        super::host::ensure_available(compose, &name)?;
        let private = super::config::state_dir(&compose.root, &name)?;
        super::config::ensure_state_scope(compose, &private)?;
        super::config::private_dir(&private)?;
        let wrapper = private.join("wrapper.sh");
        let env = private.join("host.env");
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let executable = std::env::current_exe()?;
        let context = super::host::prepare(compose, &spec, &private)?;
        let script = format!(
            "#!/bin/sh\nexec {} --root {} adapter run --context-file {} -- {}\n",
            quote(&executable.to_string_lossy()),
            quote(&compose.root.to_string_lossy()),
            quote(&context.to_string_lossy()),
            quote(&name)
        );
        team_core::persistence::atomic_write(&wrapper, script.as_bytes())?;
        let path = std::env::var("PATH").unwrap_or_default();
        anyhow::ensure!(
            !path.contains(['\n', '\r', '\0']),
            "adapter launch PATH cannot contain control characters"
        );
        let mut environment = format!("PATH={path}\n");
        for key in ["HOME", "USERPROFILE"] {
            if let Some(value) = std::env::var_os(key) {
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("adapter environment path is not UTF-8"))?;
                anyhow::ensure!(
                    !value.contains(['\n', '\r', '\0']),
                    "adapter environment path contains control characters"
                );
                environment.push_str(&format!("{key}={value}\n"));
            }
        }
        team_core::persistence::atomic_write(&env, environment.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600))?;
        }
        let launch = AgentSpec {
            wrapper,
            env_file: env,
            ..agent_spec(compose, &name, &session)
        };
        TmuxSupervisor.up(&launch)?;
        applied.insert(
            name.clone(),
            Applied {
                session,
                agent: spec.config.agent.clone(),
                fingerprint: desired_fingerprint,
                state_dir: Some(private),
            },
        );
        save(&compose.root, &applied)?;
        println!("started · adapter {name}");
    }
    Ok(())
}
fn fingerprint(compose: &Compose, spec: &Spec) -> Result<String> {
    let source = if spec.kind == "whatsapp" {
        super::assets::fingerprint()
    } else {
        String::new()
    };
    let input = serde_json::to_vec(&(
        spec.fingerprint()?,
        &compose.global.broker.path,
        &compose.global.supervisor.tmux_prefix,
        env!("CARGO_PKG_VERSION"),
        source,
    ))?;
    Ok(blake3::hash(&input).to_hex().to_string())
}

fn stop(compose: &Compose, name: &str, session: &str) -> Result<()> {
    super::host::request_stop(compose, name)?;
    let spec = agent_spec(compose, name, session);
    TmuxSupervisor.drain(&spec, std::time::Duration::from_secs(8))?;
    // A dead pane can retain its tmux session until explicit removal.
    TmuxSupervisor.down(&spec)
}
