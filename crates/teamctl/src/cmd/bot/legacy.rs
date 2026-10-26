use super::*;
use std::collections::{BTreeMap, BTreeSet};
use team_core::supervisor::{AgentSpec, AgentState, Supervisor};

/// Older releases did not persist bot identities. A matching session name
/// alone is insufficient: a project named `bot-p` can occupy `bot-p-m`.
/// Recover only candidates whose launch command proves the root/manager.
pub(super) fn discover(
    root: &Path,
    supervisor: &impl Supervisor,
) -> Result<BTreeMap<String, String>> {
    discover_with(root, supervisor, &owned_bot_command)
}

fn discover_with(
    root: &Path,
    supervisor: &impl Supervisor,
    owner: &impl Fn(&Path, &str, &str, &BTreeSet<PathBuf>) -> Result<bool>,
) -> Result<BTreeMap<String, String>> {
    let mut saved = super::supervisor::load_sessions(root)?;
    let mut candidates = BTreeMap::new();
    let mut agent_sessions = BTreeSet::new();
    let mut mailboxes = BTreeSet::new();
    let native_agents: BTreeSet<String> = Compose::load_read_only(root)
        .ok()
        .map(|c| {
            c.agents()
                .filter(|h| {
                    h.spec
                        .execution
                        .as_ref()
                        .is_some_and(|e| e.mode == team_core::compose::ExecutionMode::Native)
                })
                .map(|h| h.id())
                .collect()
        })
        .unwrap_or_default();
    let mut add_agent = |id: String, session: String, candidate: bool| {
        agent_sessions.insert(session.replace('.', "_"));
        if !candidate || native_agents.contains(&id) {
            return;
        }
        let suffix = id.replace(':', "-");
        if let Some(prefix) = session.strip_suffix(&suffix) {
            candidates.insert((id.clone(), bot_session_name(prefix, &id)), ());
        }
    };
    if let Some(snapshot) = super::super::snapshot::read(root) {
        mailboxes.insert(root.join(snapshot.global.broker_path));
        for (id, entry) in snapshot.agents {
            let native = fs::read_to_string(&entry.env_file)
                .ok()
                .is_some_and(|env| env.lines().any(|line| line == "EXECUTION_MODE=native"));
            add_agent(id, entry.tmux_session, !native);
        }
    }
    if let Ok(compose) = Compose::load_read_only(root) {
        mailboxes.insert(root.join(&compose.global.broker.path));
        for h in compose.agents() {
            let session = format!(
                "{}{}-{}",
                compose.global.supervisor.tmux_prefix, h.project, h.agent
            );
            add_agent(h.id(), session, true);
        }
    }
    if let Some(directory) = team_core::registry::config_dir() {
        let registry = team_core::registry::load(&directory)?;
        for row in registry.roster_for_root(root) {
            add_agent(row.id(), row.tmux_session, true);
        }
        // Known agents in any root take precedence over a guessed legacy
        // bot identity, including pre-upgrade namespace collisions.
        for team in registry.teams {
            for agent in &team.agents {
                agent_sessions.insert(
                    format!("{}{}-{agent}", team.tmux_prefix, team.project_id).replace('.', "_"),
                );
            }
            if team.root != root {
                if let Some(snapshot) = super::super::snapshot::read(&team.root) {
                    agent_sessions.extend(
                        snapshot
                            .agents
                            .into_values()
                            .map(|entry| entry.tmux_session.replace('.', "_")),
                    );
                }
            }
        }
    }
    for ((id, session), ()) in candidates {
        if saved.contains_key(&id) || agent_sessions.contains(&session.replace('.', "_")) {
            continue;
        }
        let (project, agent) = id.split_once(':').unwrap_or((&id, ""));
        let spec = AgentSpec {
            project: project.into(),
            agent: agent.into(),
            tmux_session: session.clone(),
            wrapper: PathBuf::new(),
            cwd: root.into(),
            env_file: PathBuf::new(),
        };
        if supervisor.state(&spec)? == AgentState::Running
            && owner(root, &id, &session, &mailboxes)?
        {
            saved.insert(id, session);
        }
    }
    Ok(saved)
}

fn owned_bot_command(
    root: &Path,
    manager: &str,
    session: &str,
    mailboxes: &BTreeSet<PathBuf>,
) -> Result<bool> {
    let target = format!("={}:0.0", session.replace('.', "_"));
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            &target,
            "#{pane_start_command}",
        ])
        .output()
        .context("read legacy bot launch identity")?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("can't find") || error.contains("no server running") {
            return Ok(false);
        }
        bail!("cannot verify legacy bot ownership: {}", error.trim());
    }
    Ok(command_is_owned(
        &String::from_utf8_lossy(&output.stdout),
        root,
        manager,
        mailboxes,
    ))
}

fn command_is_owned(
    command: &str,
    root: &Path,
    manager: &str,
    mailboxes: &BTreeSet<PathBuf>,
) -> bool {
    let Some(mut argv) = shlex::split(command) else {
        return false;
    };
    for _ in 0..3 {
        let Some(program) = argv.first() else {
            return false;
        };
        let name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(name, "sh" | "bash" | "dash" | "zsh")
            && argv.get(1).is_some_and(|arg| arg == "-c")
        {
            let Some(nested) = argv.get(2).and_then(|nested| shlex::split(nested)) else {
                return false;
            };
            argv = nested;
            continue;
        }
        if name != "team-bot" && Path::new(program) != team_bot_bin() {
            return false;
        }
        let argument = |flag: &str| {
            argv.windows(2)
                .find(|pair| pair[0] == flag)
                .map(|pair| pair[1].as_str())
        };
        let Some(mailbox) = argument("--mailbox").map(Path::new) else {
            return false;
        };
        return argument("--manager") == Some(manager)
            && (mailboxes.contains(mailbox) || mailbox.starts_with(root));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::up::regression_tests::{fixture, FakeSupervisor};

    #[test]
    fn removed_legacy_interface_is_discovered_from_last_applied_identity() {
        let (_dir, mut compose) = fixture();
        compose.global.supervisor.tmux_prefix = "t-".into();
        compose.projects[0]
            .managers
            .get_mut("a")
            .unwrap()
            .interfaces = Some(
            serde_yaml::from_str("telegram:\n  bot_token_env: TOKEN\n  chat_ids_env: CHATS\n")
                .unwrap(),
        );
        let previous = super::super::super::snapshot::compute(&compose, "team-mcp");
        super::super::super::snapshot::write(&compose.root, &previous).unwrap();
        compose.projects[0]
            .managers
            .get_mut("a")
            .unwrap()
            .interfaces = None;
        let supervisor = FakeSupervisor::default();
        supervisor.running.borrow_mut().insert("t-bot-p-a".into());
        assert!(!compose.root.join("state/bots.json").exists());
        let recovered =
            discover_with(&compose.root, &supervisor, &|_, id, _, _| Ok(id == "p:a")).unwrap();
        assert_eq!(recovered["p:a"], "t-bot-p-a");
        assert!(
            !compose.root.join("state/bots.json").exists(),
            "planning stays read-only"
        );
        let plan = super::super::reconcile::plan_from_saved(
            &compose,
            None,
            None,
            false,
            &supervisor,
            recovered,
        )
        .unwrap();
        assert!(
            !plan.is_empty(),
            "removed legacy interface must have a bot stop"
        );
        super::super::reconcile::stop_bots_with(&plan, &compose.root, &supervisor).unwrap();
        assert_eq!(*supervisor.stopped.borrow(), ["t-bot-p-a"]);
    }

    #[test]
    fn an_agent_with_a_bot_shaped_name_is_never_guessed_to_be_a_legacy_bot() {
        let (_dir, mut compose) = fixture();
        compose.global.supervisor.tmux_prefix = "t-".into();
        let mut snapshot = super::super::super::snapshot::compute(&compose, "team-mcp");
        let mut entry = snapshot.agents["p:a"].clone();
        entry.tmux_session = "t-bot-p-a".into();
        snapshot.agents.insert("bot-p:a".into(), entry);
        super::super::super::snapshot::write(&compose.root, &snapshot).unwrap();
        let supervisor = FakeSupervisor::default();
        supervisor.running.borrow_mut().insert("t-bot-p-a".into());
        let recovered = discover_with(&compose.root, &supervisor, &|_, _, _, _| {
            panic!("an agent identity must not be considered a bot")
        })
        .unwrap();
        assert!(recovered.is_empty());
    }

    #[test]
    fn down_reaps_a_verified_legacy_bot_without_an_identity_file() {
        let (_dir, mut compose) = fixture();
        compose.global.supervisor.tmux_prefix = "old-".into();
        super::super::super::snapshot::write(
            &compose.root,
            &super::super::super::snapshot::compute(&compose, "team-mcp"),
        )
        .unwrap();
        let supervisor = FakeSupervisor::default();
        supervisor.running.borrow_mut().insert("old-bot-p-a".into());
        let recovered =
            discover_with(&compose.root, &supervisor, &|_, id, _, _| Ok(id == "p:a")).unwrap();
        super::super::supervisor::down_orphans_with(
            &compose.root,
            &BTreeSet::new(),
            Some("p"),
            &supervisor,
            recovered,
        )
        .unwrap();
        assert_eq!(*supervisor.stopped.borrow(), ["old-bot-p-a"]);
        assert!(super::super::supervisor::load_sessions(&compose.root)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn legacy_command_requires_exact_manager_and_mailbox_ownership() {
        let root = Path::new("/team/.team");
        let mailboxes = BTreeSet::from([root.join("state/mailbox.db")]);
        let command = "sh -c 'team-bot --mailbox /team/.team/state/mailbox.db --token fixture --manager p:a --tmux-prefix t-'";
        assert!(command_is_owned(command, root, "p:a", &mailboxes));
        assert!(!command_is_owned(command, root, "p:b", &mailboxes));
        assert!(!command_is_owned(
            "codex --mailbox /team/.team/state/mailbox.db --manager p:a",
            root,
            "p:a",
            &mailboxes
        ));
        assert!(!command_is_owned(
            "team-bot --mailbox /other/state/mailbox.db --manager p:a",
            root,
            "p:a",
            &mailboxes
        ));
    }
}
