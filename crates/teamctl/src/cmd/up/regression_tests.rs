use super::*;
use crate::cmd::snapshot;
use std::cell::RefCell;
use std::collections::HashSet;

pub(crate) fn fixture() -> (tempfile::TempDir, Compose) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("projects")).unwrap();
    fs::write(
        dir.path().join("team-compose.yaml"),
        "version: \"2.0.0\"\nprojects:\n  - file: projects/p.yaml\n  - file: projects/q.yaml\n",
    )
    .unwrap();
    for project in ["p", "q"] {
        fs::write(dir.path().join(format!("projects/{project}.yaml")), format!("version: 2\nproject:\n  id: {project}\n  name: {project}\n  cwd: .\nmanagers:\n  a:\n    runtime: codex\nworkers:\n  b:\n    runtime: codex\nchannels:\n  - name: all\n    members: '*'\n")).unwrap();
    }
    let compose = Compose::load(dir.path()).unwrap();
    (dir, compose)
}

#[derive(Default)]
pub(crate) struct FakeSupervisor {
    pub running: RefCell<HashSet<String>>,
    pub spawned: RefCell<Vec<String>>,
    pub stopped: RefCell<Vec<String>>,
    pub fail_agent: Option<String>,
}

impl Supervisor for FakeSupervisor {
    fn up(&self, spec: &AgentSpec) -> Result<()> {
        anyhow::ensure!(
            self.fail_agent.as_deref() != Some(&spec.agent),
            "fake spawn failure"
        );
        self.spawned
            .borrow_mut()
            .push(format!("{}:{}", spec.project, spec.agent));
        self.running.borrow_mut().insert(spec.tmux_session.clone());
        Ok(())
    }
    fn down(&self, spec: &AgentSpec) -> Result<()> {
        self.stopped.borrow_mut().push(spec.tmux_session.clone());
        self.running.borrow_mut().remove(&spec.tmux_session);
        Ok(())
    }
    fn state(&self, spec: &AgentSpec) -> Result<AgentState> {
        Ok(if self.running.borrow().contains(&spec.tmux_session) {
            AgentState::Running
        } else {
            AgentState::Stopped
        })
    }
}

#[test]
fn up_keeps_running_agents_last_applied_inputs() {
    let (_dir, mut compose) = fixture();
    let previous = snapshot::compute(&compose, "team-mcp");
    compose.projects[0].managers.get_mut("a").unwrap().model = Some("new-model".into());
    let next = snapshot::compute(&compose, "team-mcp");
    let supervisor = FakeSupervisor::default();
    supervisor
        .running
        .borrow_mut()
        .insert(previous.agents["p:a"].tmux_session.clone());
    let mut applied = previous.clone();
    start_agents_with(
        &compose,
        &supervisor,
        Some(&previous),
        Some("p"),
        None,
        false,
        |id| {
            applied = snapshot::merge_applied(Some(&applied), &next, [id.to_string()], []);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(supervisor.spawned.borrow().as_slice(), ["p:b"]);
    assert_eq!(
        applied.agents["p:a"].fingerprints,
        previous.agents["p:a"].fingerprints
    );
    assert_ne!(
        applied.agents["p:a"].fingerprints,
        next.agents["p:a"].fingerprints
    );
}

#[test]
fn up_does_not_duplicate_live_session_after_prefix_change() {
    let (_dir, mut compose) = fixture();
    let previous = snapshot::compute(&compose, "team-mcp");
    compose.global.supervisor.tmux_prefix = "new-".into();
    let supervisor = FakeSupervisor::default();
    supervisor
        .running
        .borrow_mut()
        .insert(previous.agents["p:a"].tmux_session.clone());
    let targets = BTreeSet::from(["a".to_string()]);
    let mut checkpoints = 0;
    start_agents_with(
        &compose,
        &supervisor,
        Some(&previous),
        Some("p"),
        Some(&targets),
        false,
        |_| {
            checkpoints += 1;
            Ok(())
        },
    )
    .unwrap();
    assert!(supervisor.spawned.borrow().is_empty());
    assert_eq!(checkpoints, 0);
}

#[test]
fn partial_spawn_failure_preserves_successful_checkpoint_only() {
    let (_dir, compose) = fixture();
    let supervisor = FakeSupervisor {
        fail_agent: Some("b".into()),
        ..Default::default()
    };
    let next = snapshot::compute(&compose, "team-mcp");
    let mut applied = None;
    let result = start_agents_with(&compose, &supervisor, None, Some("p"), None, false, |id| {
        let snap = snapshot::merge_applied(applied.as_ref(), &next, [id.to_string()], []);
        snapshot::write(&compose.root, &snap)?;
        applied = Some(snap);
        Ok(())
    });
    assert!(result.is_err());
    let stored = snapshot::read(&compose.root).unwrap();
    assert_eq!(
        stored.agents.keys().map(String::as_str).collect::<Vec<_>>(),
        ["p:a"]
    );
}

#[test]
fn scoped_first_apply_creates_managed_scripts_and_mailbox_roster() {
    let (_dir, compose) = fixture();
    ensure_wrapper_and_dirs(&compose).unwrap();
    render_project_public(&compose, "p").unwrap();
    register_project_public(&compose, "p").unwrap();
    assert!(compose.root.join("bin/agent-wrapper.sh").is_file());
    assert!(team_core::render::env_path(&compose.root, "p", "a").is_file());
    assert!(!team_core::render::env_path(&compose.root, "q", "a").exists());
    let conn = rusqlite::Connection::open(compose.root.join(&compose.global.broker.path)).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agents", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn registration_prunes_live_roster_without_erasing_history_or_other_project() {
    let (_dir, mut compose) = fixture();
    register_all_public(&compose).unwrap();
    let conn = rusqlite::Connection::open(compose.root.join(&compose.global.broker.path)).unwrap();
    conn.execute("INSERT INTO messages(project_id, sender, recipient, text, sent_at) VALUES('p', 'p:b', 'channel:p:all', 'history', 1)", []).unwrap();
    let message_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO message_receipts(message_id, agent_id, acked_at) VALUES(?1, 'p:b', 2)",
        [message_id],
    )
    .unwrap();
    compose.projects[0].workers.clear();
    compose.projects[0].channels.clear();
    register_project_public(&compose, "p").unwrap();
    let count = |table: &str| {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap()
    };
    assert_eq!(count("agents"), 3);
    assert_eq!(count("channels"), 1);
    assert_eq!(count("messages"), 1);
    assert_eq!(count("message_receipts"), 1);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM channel_members WHERE channel_id='q:all'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
fn project_only_metadata_changes_refresh_mailbox_without_advancing_agent_inputs() {
    let (_dir, compose) = fixture();
    register_all_public(&compose).unwrap();
    let previous = snapshot::compute(&compose, "team-mcp");
    let path = compose.root.join("projects/p.yaml");
    let raw = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        raw.replace("name: p", "name: Renamed project")
            .replace("members: '*'", "members: [a]"),
    )
    .unwrap();
    let current = Compose::load(&compose.root).unwrap();
    let next = snapshot::compute(&current, "team-mcp");
    let plan = snapshot::plan(Some(&previous), &next);
    assert!(
        plan.is_empty(),
        "metadata does not restart unchanged agents"
    );
    assert_ne!(
        previous.compose_digest, next.compose_digest,
        "reload must register metadata"
    );
    register_project_public(&current, "p").unwrap();
    let conn = rusqlite::Connection::open(current.root.join(&current.global.broker.path)).unwrap();
    let name: String = conn
        .query_row("SELECT name FROM projects WHERE id='p'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Renamed project");
    let members: usize = conn
        .query_row(
            "SELECT count(*) FROM channel_members WHERE channel_id='p:all'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(members, 1);
    let applied = snapshot::merge_applied(Some(&previous), &next, [], []);
    snapshot::write(&current.root, &applied).unwrap();
    let stored = snapshot::read(&current.root).unwrap();
    assert_eq!(stored.compose_digest, next.compose_digest);
    assert_eq!(
        stored.agents["p:a"].fingerprints,
        previous.agents["p:a"].fingerprints
    );
}

#[test]
fn concurrent_roots_cannot_reserve_the_same_project_identity() {
    let (_first_dir, first) = fixture();
    let (_second_dir, second) = fixture();
    let config = tempfile::tempdir().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [first, second]
        .into_iter()
        .map(|compose| {
            let path = config.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || -> Result<()> {
                barrier.wait();
                let _lock = team_core::persistence::ProcessLock::acquire(
                    &path.join("lifecycle.lock.sqlite"),
                )?;
                let registry = team_core::registry::load(&path)?;
                guard_no_name_collision_in(&registry, &compose, Some("p"), &Path::exists)?;
                team_core::registry::upsert_many(
                    &path,
                    registry_entries(
                        compose.agents().map(|h| (h.project, h.agent)),
                        &compose.root,
                        &compose.global.supervisor.tmux_prefix,
                        Some("p"),
                        "now",
                    ),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        team_core::registry::load(config.path())
            .unwrap()
            .teams
            .len(),
        1
    );
    assert!(results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| e.to_string().contains("already up")));
}

#[test]
fn different_roots_cannot_reserve_colliding_concrete_agent_or_bot_names() {
    for (first_id, first_agent, second_id, second_agent, bot) in [
        ("a-b", "c", "a", "b-c", false),
        ("p.one", "a", "p_one", "a", false),
        ("p", "a", "bot-p", "a", true),
    ] {
        let (_first_dir, mut first) = fixture();
        let (_second_dir, mut second) = fixture();
        for (compose, project, agent) in [
            (&mut first, first_id, first_agent),
            (&mut second, second_id, second_agent),
        ] {
            compose.projects[0].project.id = project.into();
            let spec = compose.projects[0].managers.remove("a").unwrap();
            compose.projects[0].managers.insert(agent.into(), spec);
        }
        if bot {
            first.projects[0]
                .managers
                .get_mut(first_agent)
                .unwrap()
                .interfaces = Some(
                serde_yaml::from_str("telegram:\n  bot_token_env: TOKEN\n  chat_ids_env: CHATS\n")
                    .unwrap(),
            );
        }
        let registry_dir = tempfile::tempdir().unwrap();
        record_in_registry_in(registry_dir.path(), &first, Some(first_id));
        let registry = team_core::registry::load(registry_dir.path()).unwrap();
        let error = guard_no_name_collision_in(&registry, &second, Some(second_id), &Path::exists)
            .unwrap_err();
        assert!(error.to_string().contains("tmux session"), "{error}");
    }
}
