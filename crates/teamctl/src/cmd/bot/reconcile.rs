use super::*;
use std::collections::BTreeMap;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

/// A plan captures old identities before manager snapshot checkpoints can
/// change their prefix. Stops run before manager restarts; starts run after.
pub(crate) struct BotPlan {
    stop: Vec<(String, AgentSpec)>,
    launch: Vec<BotSpec>,
}

impl BotPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.stop.is_empty() && self.launch.is_empty()
    }

    pub(crate) fn print(&self, dry_run: bool) {
        let suffix = if dry_run { " (dry run)" } else { "" };
        for (id, _) in &self.stop {
            println!("removed · bot {id}{suffix}");
        }
        for spec in &self.launch {
            println!("started · bot {}{suffix}", spec.manager);
        }
    }
}

pub(crate) fn plan_bots(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    force: bool,
) -> Result<BotPlan> {
    plan_bots_with(compose, project, targets, force, &TmuxSupervisor)
}

fn plan_bots_with(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    force: bool,
    sup: &impl Supervisor,
) -> Result<BotPlan> {
    let saved = super::legacy::discover(&compose.root, sup)?;
    plan_from_saved(compose, project, targets, force, sup, saved)
}

pub(super) fn plan_from_saved(
    compose: &Compose,
    project: Option<&str>,
    targets: Option<&BTreeSet<String>>,
    force: bool,
    sup: &impl Supervisor,
    saved: BTreeMap<String, String>,
) -> Result<BotPlan> {
    let inputs = load_inputs(&compose.root)?;
    let specs = bot_specs(compose);
    let desired: BTreeSet<_> = specs.iter().map(|spec| spec.manager.clone()).collect();
    let in_scope = |id: &str| {
        id.split_once(':').is_some_and(|(p, agent)| {
            project.is_none_or(|scope| p == scope)
                && targets.is_none_or(|selected| selected.contains(agent))
        })
    };
    let mut plan = BotPlan {
        stop: vec![],
        launch: vec![],
    };
    for (id, session) in &saved {
        if in_scope(id) && !desired.contains(id) {
            let (p, agent) = id.split_once(':').unwrap_or((id, ""));
            plan.stop.push((
                id.clone(),
                AgentSpec {
                    project: p.into(),
                    agent: agent.into(),
                    tmux_session: session.clone(),
                    wrapper: PathBuf::new(),
                    cwd: compose.root.clone(),
                    env_file: PathBuf::new(),
                },
            ));
        }
    }
    for spec in specs.into_iter().filter(|spec| in_scope(&spec.manager)) {
        let observed = saved
            .get(&spec.manager)
            .cloned()
            .unwrap_or_else(|| super::supervisor::legacy_session(&spec, &compose.root));
        let mut old = super::supervisor::session_spec(&spec, &compose.root, &observed);
        let mut running = sup.state(&old)? == AgentState::Running;
        if !running && observed != spec.session {
            old = super::supervisor::session_spec(&spec, &compose.root, &spec.session);
            running = sup.state(&old)? == AgentState::Running;
        }
        if force
            || !running
            || inputs.get(&spec.manager) != Some(&fingerprint(&spec, &compose.root)?)
        {
            if running {
                plan.stop.push((spec.manager.clone(), old));
            }
            plan.launch.push(spec);
        }
    }
    Ok(plan)
}

pub(crate) fn stop_bots(plan: &BotPlan, root: &Path) -> Result<()> {
    stop_bots_with(plan, root, &TmuxSupervisor)
}

pub(super) fn stop_bots_with(
    plan: &BotPlan,
    root: &Path,
    supervisor: &impl Supervisor,
) -> Result<()> {
    let mut saved = super::supervisor::load_sessions(root)?;
    for (id, spec) in &plan.stop {
        supervisor.down(spec)?;
        saved.remove(id);
        super::supervisor::save_sessions(root, &saved)?;
        forget_input(root, id)?;
    }
    Ok(())
}

pub(crate) fn start_bots(plan: &BotPlan, root: &Path) -> Result<()> {
    let binary = team_bot_bin();
    start_bots_with(plan, root, |spec| up_one(spec, &binary, root))
}

fn start_bots_with(
    plan: &BotPlan,
    root: &Path,
    mut launch: impl FnMut(&BotSpec) -> Result<bool>,
) -> Result<()> {
    for spec in &plan.launch {
        if launch(spec)? {
            let mut saved = super::supervisor::load_sessions(root)?;
            saved.insert(spec.manager.clone(), spec.session.clone());
            super::supervisor::save_sessions(root, &saved)?;
            record_input(root, spec)?;
            println!("up · bot {} → {}", spec.session, spec.manager);
        }
    }
    Ok(())
}

pub(super) fn record_input(root: &Path, spec: &BotSpec) -> Result<()> {
    let mut inputs = load_inputs(root)?;
    inputs.insert(spec.manager.clone(), fingerprint(spec, root)?);
    save_inputs(root, &inputs)
}

pub(super) fn forget_input(root: &Path, manager: &str) -> Result<()> {
    let mut inputs = load_inputs(root)?;
    inputs.remove(manager);
    save_inputs(root, &inputs)
}

fn fingerprint(spec: &BotSpec, root: &Path) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(&serde_json::to_vec(spec)?);
    hash.update(team_bot_bin().to_string_lossy().as_bytes());
    for variable in [
        Some(spec.token_env.as_str()),
        Some(spec.chats_env.as_str()),
        spec.stt.as_ref().map(|stt| stt.api_key_env.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        let value = env_value(root, variable).unwrap_or_default();
        hash.update(&(variable.len() as u64).to_le_bytes());
        hash.update(variable.as_bytes());
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    Ok(format!("blake3:{}", hash.finalize().to_hex()))
}

fn env_value(root: &Path, variable: &str) -> Option<String> {
    if let Ok(value) = std::env::var(variable) {
        return Some(value);
    }
    for path in [
        root.join(".env"),
        root.parent().unwrap_or(root).join(".env"),
    ] {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        for line in raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
        {
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((key, value)) = line.split_once('=') {
                if key == variable {
                    return Some(value.trim().trim_matches('"').trim_matches('\'').into());
                }
            }
        }
    }
    None
}

fn load_inputs(root: &Path) -> Result<BTreeMap<String, String>> {
    match fs::read(root.join("state/bot-inputs.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parse persisted bot inputs"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error).context("read persisted bot inputs"),
    }
}

fn save_inputs(root: &Path, inputs: &BTreeMap<String, String>) -> Result<()> {
    team_core::persistence::atomic_write(
        &root.join("state/bot-inputs.json"),
        &serde_json::to_vec_pretty(inputs)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::up::regression_tests::{fixture, FakeSupervisor};

    fn bots() -> (tempfile::TempDir, Compose) {
        let (dir, mut compose) = fixture();
        for project in &mut compose.projects {
            project.managers.get_mut("a").unwrap().interfaces = Some(serde_yaml::from_str(
                "telegram:\n  bot_token_env: TEAMCTL_TEST_BOT_TOKEN\n  chat_ids_env: TEAMCTL_TEST_BOT_CHATS\n").unwrap());
        }
        (dir, compose)
    }

    fn applied_bots(compose: &Compose, supervisor: &FakeSupervisor) {
        let specs = bot_specs(compose);
        let sessions = specs
            .iter()
            .map(|spec| (spec.manager.clone(), spec.session.clone()))
            .collect();
        super::super::supervisor::save_sessions(&compose.root, &sessions).unwrap();
        for spec in specs {
            record_input(&compose.root, &spec).unwrap();
            supervisor.running.borrow_mut().insert(spec.session);
        }
    }

    #[test]
    fn prefix_broker_and_interface_changes_reconcile_only_selected_manager_bots() {
        let (_dir, mut compose) = bots();
        let supervisor = FakeSupervisor::default();
        applied_bots(&compose, &supervisor);
        let old_p = bot_specs(&compose).remove(0).session;
        let old_q = bot_specs(&compose).remove(1).session;
        compose.global.supervisor.tmux_prefix = "new-".into();
        compose.global.broker.path = "state/other.db".into();
        let targets = BTreeSet::from(["a".to_string()]);
        let plan = plan_bots_with(&compose, Some("p"), Some(&targets), false, &supervisor).unwrap();
        assert_eq!(plan.stop.len(), 1);
        assert_eq!(plan.stop[0].1.tmux_session, old_p);
        assert_eq!(plan.launch[0].session, "new-bot-p-a");
        assert!(plan.launch[0].mailbox.ends_with("state/other.db"));
        stop_bots_with(&plan, &compose.root, &supervisor).unwrap();
        assert!(supervisor.running.borrow().contains(&old_q));
        start_bots_with(&plan, &compose.root, |spec| {
            supervisor.up(&super::super::supervisor::session_spec(
                spec,
                &compose.root,
                &spec.session,
            ))?;
            Ok(true)
        })
        .unwrap();
        assert!(
            plan_bots_with(&compose, Some("p"), Some(&targets), false, &supervisor)
                .unwrap()
                .is_empty()
        );
        compose.projects[0]
            .managers
            .get_mut("a")
            .unwrap()
            .interfaces = None;
        let removed = plan_bots_with(&compose, Some("p"), None, false, &supervisor).unwrap();
        assert_eq!(removed.stop.len(), 1);
        assert!(removed.launch.is_empty());
    }

    #[test]
    fn stopped_bots_recover_and_partial_launch_failure_keeps_completed_inputs() {
        let (_dir, compose) = bots();
        let supervisor = FakeSupervisor::default();
        let plan = plan_bots_with(&compose, None, None, false, &supervisor).unwrap();
        assert_eq!(plan.launch.len(), 2);
        let result = start_bots_with(&plan, &compose.root, |spec| {
            anyhow::ensure!(spec.manager != "q:a", "fake bot spawn failure");
            supervisor.up(&super::super::supervisor::session_spec(
                spec,
                &compose.root,
                &spec.session,
            ))?;
            Ok(true)
        });
        assert!(result.is_err());
        assert_eq!(load_inputs(&compose.root).unwrap().len(), 1);
        let retry = plan_bots_with(&compose, None, None, false, &supervisor).unwrap();
        assert_eq!(
            retry
                .launch
                .iter()
                .map(|spec| spec.manager.as_str())
                .collect::<Vec<_>>(),
            ["q:a"]
        );
    }

    #[test]
    fn dry_run_fingerprints_dotenv_credentials_without_storing_the_secret() {
        let (_dir, compose) = bots();
        let spec = bot_specs(&compose).remove(0);
        fs::write(
            compose.root.join(".env"),
            "TEAMCTL_TEST_BOT_TOKEN=first-secret\n",
        )
        .unwrap();
        record_input(&compose.root, &spec).unwrap();
        let old = load_inputs(&compose.root).unwrap()[&spec.manager].clone();
        let stored = fs::read_to_string(compose.root.join("state/bot-inputs.json")).unwrap();
        assert!(!stored.contains("first-secret"));
        fs::write(
            compose.root.join(".env"),
            "TEAMCTL_TEST_BOT_TOKEN=second-secret\n",
        )
        .unwrap();
        assert_ne!(old, fingerprint(&spec, &compose.root).unwrap());
    }
}
