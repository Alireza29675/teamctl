use super::*;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

// ── Spawn helpers (used by cmd::up / cmd::down) ─────────────────────

#[derive(serde::Serialize)]
pub struct BotSpec {
    pub interface_id: Option<String>,
    pub interface_name: Option<String>,
    pub interface_description: Option<String>,
    pub manager: String,
    /// T-367: the manager's `display_name` (T-160) when set. Passed to
    /// `team-bot` as `--manager-display-name` so the first-connect greeting
    /// reads "Connected to <name> via teamctl" with the human label instead
    /// of the bare `<project>:<manager>` id. `None` → the bot falls back to
    /// the id. Resolved here because `display_name` is render-time-only and
    /// never lands in the mailbox DB the bot reads.
    pub display_name: Option<String>,
    pub session: String,
    pub mailbox: PathBuf,
    pub token_env: String,
    pub chats_env: String,
    /// Tmux prefix the running bot needs to compute the manager's tmux
    /// session for slash-passthrough (T-086-G). Lifted from compose so a
    /// project that overrides `supervisor.tmux_prefix` carries that override
    /// through to the bot process.
    pub tmux_prefix: String,
    /// Speech-to-text settings for inbound Telegram voice notes (T-101).
    /// `None` means the bot will not handle voice messages — the default
    /// preserves prior behavior for setups that don't opt in.
    pub stt: Option<BotSttSpec>,
}

/// Resolved STT plumbing for one bot. Mirrors the env-var-name pattern
/// used by `bot_token_env` / `chat_ids_env` — the secret never lands in
/// `BotSpec`; only the var name does, and `up_one` looks it up at spawn.
#[derive(serde::Serialize)]
pub struct BotSttSpec {
    pub provider: String,
    pub api_key_env: String,
    pub model: String,
    pub language: Option<String>,
}

pub fn bot_specs(compose: &Compose) -> Vec<BotSpec> {
    let prefix = &compose.global.supervisor.tmux_prefix;
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mut out = Vec::new();
    for proj in &compose.projects {
        for (role, agent) in &proj.managers {
            if let Some(tg) = agent.telegram() {
                let mgr = format!("{}:{}", proj.project.id, role);
                let stt = tg.speech_to_text.as_ref().map(|s| BotSttSpec {
                    provider: s.provider.clone(),
                    api_key_env: s.api_key_env.clone(),
                    model: s.model.clone(),
                    language: s.language.clone(),
                });
                out.push(BotSpec {
                    interface_id: None,
                    interface_name: None,
                    interface_description: None,
                    session: bot_session_name(prefix, &mgr),
                    mailbox: mailbox.clone(),
                    token_env: tg.bot_token_env.clone(),
                    chats_env: tg.chat_ids_env.clone(),
                    manager: mgr,
                    display_name: agent.display_name.clone(),
                    tmux_prefix: prefix.clone(),
                    stt,
                });
            }
        }
    }
    for interface in compose.global.interfaces.iter().filter(|i| i.is_telegram()) {
        let Some(target) = interface.agent() else {
            continue;
        };
        let Some(agent) = compose.agents().find(|a| a.id() == target) else {
            continue;
        };
        let Some(token_env) = interface.bot_token_env() else {
            continue;
        };
        let Some(chats_env) = interface.authorized_chat_ids_env().or_else(|| {
            interface
                .config
                .get("chat_ids_env")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }) else {
            continue;
        };
        out.push(BotSpec {
            interface_id: Some(interface.instance_id().into()),
            interface_name: Some(interface.name.clone()),
            interface_description: Some(interface.description.clone()),
            manager: target,
            display_name: agent.spec.display_name.clone(),
            session: format!("{prefix}bot-interface-{}", interface.instance_id()).replace('.', "_"),
            mailbox: mailbox.clone(),
            token_env,
            chats_env,
            tmux_prefix: prefix.clone(),
            stt: None,
        });
    }
    out
}

impl BotSpec {
    pub fn identity(&self) -> String {
        self.interface_id.as_ref().map_or_else(
            || self.manager.clone(),
            |id| format!("{}@{}", self.manager, id),
        )
    }
}

/// Spawn one tmux session running `team-bot` for this manager.
/// No-op if already running. Skips and warns when env vars are unset.
pub fn up_one(spec: &BotSpec, team_bot_bin: &Path, root: &Path) -> Result<bool> {
    if team_core::environment::lookup(Some(root), &spec.token_env)?
        .is_none_or(|value| value.trim().is_empty())
    {
        eprintln!(
            "skip · bot {} ({} unset — run `teamctl bot setup`)",
            spec.session, spec.token_env
        );
        return Ok(false);
    }
    let chats = team_core::environment::lookup(Some(root), &spec.chats_env)?.unwrap_or_default();

    let mut saved = load_sessions(root)?;
    let fallback = legacy_session(spec, root);
    let observed = saved.get(&spec.identity()).unwrap_or(&fallback);
    let supervisor = TmuxSupervisor;
    if supervisor.state(&session_spec(spec, root, observed))? == AgentState::Running {
        return Ok(true);
    }
    if observed != &spec.session
        && supervisor.state(&session_spec(spec, root, &spec.session))? == AgentState::Running
    {
        saved.insert(spec.identity(), spec.session.clone());
        save_sessions(root, &saved)?;
        return Ok(true);
    }

    let stt_enabled = if let Some(stt) = &spec.stt {
        let enabled = team_core::environment::lookup(Some(root), &stt.api_key_env)?
            .is_some_and(|value| !value.trim().is_empty());
        if !enabled {
            eprintln!(
                "skip voice · bot {} ({} unset — voice messages will be ignored)",
                spec.session, stt.api_key_env
            );
        }
        enabled
    } else {
        false
    };
    let cmd = launch_command(spec, team_bot_bin, root, &chats, stt_enabled);
    // -x/-y match the agent supervisor: keep the detached pane large enough
    // that inner TUIs (if anything ever runs interactively here) don't get
    // wedged into the tmux 80x24 default. Bot is non-interactive today, but
    // symmetry with `team-core::supervisor` matters when an operator does
    // attach for debugging.
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-x",
            "200",
            "-y",
            "50",
            "-s",
            &spec.session,
            "-c",
            &root.display().to_string(),
            "sh",
            "-c",
            &cmd,
        ])
        .status()
        .context("spawn tmux new-session for bot")?;
    anyhow::ensure!(status.success(), "tmux new-session exited {status}");
    saved.insert(spec.identity(), spec.session.clone());
    save_sessions(root, &saved)?;
    super::reconcile::record_input(root, spec)?;
    Ok(true)
}

/// Render references only: bot/STT credentials resolve after the bot process starts.
fn launch_command(
    spec: &BotSpec,
    binary: &Path,
    root: &Path,
    chats: &str,
    stt_enabled: bool,
) -> String {
    let display_name = spec
        .display_name
        .as_ref()
        .map(|name| format!(" --manager-display-name {}", shlex_quote(name)))
        .unwrap_or_default();
    let interface = spec
        .interface_id
        .as_ref()
        .map(|id| {
            format!(
                " --interface-id {} --interface-name {} --interface-description {}",
                shlex_quote(id),
                shlex_quote(spec.interface_name.as_deref().unwrap_or(id)),
                shlex_quote(spec.interface_description.as_deref().unwrap_or(""))
            )
        })
        .unwrap_or_default();
    let stt = spec
        .stt
        .as_ref()
        .filter(|_| stt_enabled)
        .map(|stt| {
            let mut flags = format!(
                " --stt-provider {} --stt-api-key-env {} --stt-model {}",
                shlex_quote(&stt.provider),
                shlex_quote(&stt.api_key_env),
                shlex_quote(&stt.model)
            );
            if let Some(language) = &stt.language {
                flags.push_str(&format!(" --stt-language {}", shlex_quote(language)));
            }
            flags
        })
        .unwrap_or_default();
    format!("{binary} --mailbox {mailbox} --token-env {token_env} --authorized-chat-ids {chats} --manager {agent} --compose-root {root} --tmux-prefix {prefix}{display_name}{interface}{stt}",binary=shlex_quote(&binary.display().to_string()),mailbox=shlex_quote(&spec.mailbox.display().to_string()),token_env=shlex_quote(&spec.token_env),chats=shlex_quote(chats),agent=shlex_quote(&spec.manager),root=shlex_quote(&root.display().to_string()),prefix=shlex_quote(&spec.tmux_prefix))
}

pub fn down_one(spec: &BotSpec, root: &Path) -> Result<()> {
    down_one_with(spec, root, &TmuxSupervisor)
}

pub(super) fn observed_state(spec: &BotSpec, root: &Path) -> Result<(String, AgentState)> {
    let saved = load_sessions(root)?;
    let session = saved
        .get(&spec.identity())
        .cloned()
        .unwrap_or_else(|| legacy_session(spec, root));
    let state = TmuxSupervisor.state(&session_spec(spec, root, &session))?;
    Ok((session, state))
}

fn down_one_with(spec: &BotSpec, root: &Path, supervisor: &impl Supervisor) -> Result<()> {
    let mut saved = load_sessions(root)?;
    let fallback = legacy_session(spec, root);
    let session = saved.get(&spec.identity()).unwrap_or(&fallback);
    supervisor.down(&session_spec(spec, root, session))?;
    saved.remove(&spec.identity());
    save_sessions(root, &saved)?;
    super::reconcile::forget_input(root, &spec.identity())
}

/// Persisted identities also cover managers or Telegram interfaces removed
/// from the compose since startup.
pub fn down_orphans(
    root: &Path,
    desired: &BTreeSet<String>,
    project: Option<&str>,
    per_agent: bool,
) -> Result<()> {
    if per_agent {
        return Ok(());
    }
    let saved = super::legacy::discover(root, &TmuxSupervisor)?;
    down_orphans_with(root, desired, project, &TmuxSupervisor, saved)
}

pub(super) fn down_orphans_with(
    root: &Path,
    desired: &BTreeSet<String>,
    project: Option<&str>,
    supervisor: &impl Supervisor,
    mut saved: std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let obsolete: Vec<_> = saved
        .iter()
        .filter(|(id, _)| {
            !desired.contains(*id)
                && project.is_none_or(|scope| id.split_once(':').is_some_and(|(p, _)| p == scope))
        })
        .map(|(id, session)| (id.clone(), session.clone()))
        .collect();
    for (id, session) in obsolete {
        let (project, agent) = id.split_once(':').unwrap_or((&id, ""));
        let spec = AgentSpec {
            project: project.into(),
            agent: agent.into(),
            tmux_session: session,
            wrapper: PathBuf::new(),
            cwd: root.into(),
            env_file: PathBuf::new(),
        };
        supervisor.down(&spec)?;
        saved.remove(&id);
        save_sessions(root, &saved)?;
        super::reconcile::forget_input(root, &id)?;
        println!("reaped · bot {id}");
    }
    Ok(())
}

pub(super) fn session_spec(spec: &BotSpec, root: &Path, session: &str) -> AgentSpec {
    let (project, agent) = spec.manager.split_once(':').unwrap_or((&spec.manager, ""));
    AgentSpec {
        project: project.into(),
        agent: agent.into(),
        tmux_session: session.into(),
        wrapper: PathBuf::new(),
        cwd: root.into(),
        env_file: PathBuf::new(),
    }
}

pub(super) fn legacy_session(spec: &BotSpec, root: &Path) -> String {
    let suffix = spec.manager.replace(':', "-");
    super::super::snapshot::read(root)
        .and_then(|snapshot| {
            snapshot
                .agents
                .get(&spec.identity())
                .and_then(|entry| entry.tmux_session.strip_suffix(&suffix))
                .map(|prefix| bot_session_name(prefix, &spec.manager))
        })
        .unwrap_or_else(|| spec.session.clone())
}

pub(super) fn load_sessions(root: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let path = root.join("state/bots.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parse persisted bot identities"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error).context("read persisted bot identities"),
    }
}

pub(super) fn save_sessions(
    root: &Path,
    sessions: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    team_core::persistence::atomic_write(
        &root.join("state/bots.json"),
        &serde_json::to_vec_pretty(sessions)?,
    )?;
    Ok(())
}

pub fn team_bot_bin() -> PathBuf {
    if let Ok(p) = std::env::var("TEAMCTL_TEAM_BOT") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join(if cfg!(windows) {
                "team-bot.exe"
            } else {
                "team-bot"
            });
            if c.exists() {
                return c;
            }
        }
    }
    PathBuf::from("team-bot")
}

pub(super) fn shlex_quote(s: &str) -> String {
    shlex::try_quote(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| format!("'{}'", s.replace('\'', "'\\''")))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn legacy_bot_identity_uses_managers_last_applied_prefix() {
        let (_dir, mut compose) = crate::cmd::up::regression_tests::fixture();
        compose.projects[0]
            .managers
            .get_mut("a")
            .unwrap()
            .interfaces = Some(
            serde_yaml::from_str("telegram:\n  bot_token_env: TOKEN\n  chat_ids_env: CHATS\n")
                .unwrap(),
        );
        let previous = bot_specs(&compose).remove(0);
        crate::cmd::snapshot::write(
            &compose.root,
            &crate::cmd::snapshot::compute(&compose, "team-mcp"),
        )
        .unwrap();
        compose.global.supervisor.tmux_prefix = "new-".into();
        let current = bot_specs(&compose).remove(0);
        assert_ne!(previous.session, current.session);
        assert_eq!(legacy_session(&current, &compose.root), previous.session);
    }

    #[test]
    fn bot_teardown_uses_persisted_identity_and_keeps_failed_teardown_for_retry() {
        struct Fake {
            stopped: RefCell<Vec<String>>,
            fail: bool,
        }
        impl Supervisor for Fake {
            fn up(&self, _: &AgentSpec) -> Result<()> {
                unreachable!()
            }
            fn state(&self, _: &AgentSpec) -> Result<AgentState> {
                Ok(AgentState::Running)
            }
            fn down(&self, spec: &AgentSpec) -> Result<()> {
                self.stopped.borrow_mut().push(spec.tmux_session.clone());
                anyhow::ensure!(!self.fail, "kill denied");
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let spec = BotSpec {
            interface_id: None,
            interface_name: None,
            interface_description: None,
            manager: "p:m".into(),
            session: "new-bot-p-m".into(),
            display_name: None,
            mailbox: PathBuf::new(),
            token_env: "TOKEN".into(),
            chats_env: "CHATS".into(),
            tmux_prefix: "new-".into(),
            stt: None,
        };
        save_sessions(
            root.path(),
            &std::collections::BTreeMap::from([("p:m".into(), "old-bot-p-m".into())]),
        )
        .unwrap();
        let failing = Fake {
            stopped: RefCell::new(vec![]),
            fail: true,
        };
        assert!(down_one_with(&spec, root.path(), &failing).is_err());
        assert_eq!(load_sessions(root.path()).unwrap()["p:m"], "old-bot-p-m");
        let successful = Fake {
            stopped: RefCell::new(vec![]),
            fail: false,
        };
        down_one_with(&spec, root.path(), &successful).unwrap();
        assert_eq!(*successful.stopped.borrow(), ["old-bot-p-m"]);
        assert!(load_sessions(root.path()).unwrap().is_empty());
    }
}

#[cfg(test)]
mod credential_reference_tests {
    use super::*;
    #[test]
    fn managed_launch_command_contains_reference_names_and_no_secret_values() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".env"),
            "BOT_TOKEN=fake-bot-secret-canary\nVOICE_KEY=fake-stt-secret-canary\n",
        )
        .unwrap();
        let spec = BotSpec {
            interface_id: None,
            interface_name: None,
            interface_description: None,
            manager: "p:agent".into(),
            display_name: None,
            session: "bot-p-agent".into(),
            mailbox: root.path().join("state/mailbox.db"),
            token_env: "BOT_TOKEN".into(),
            chats_env: "CHATS".into(),
            tmux_prefix: "a-".into(),
            stt: Some(BotSttSpec {
                provider: "groq".into(),
                api_key_env: "VOICE_KEY".into(),
                model: "whisper".into(),
                language: None,
            }),
        };
        let command = launch_command(&spec, Path::new("team-bot"), root.path(), "7", true);
        assert!(command.contains("--token-env BOT_TOKEN"));
        assert!(command.contains("--stt-api-key-env VOICE_KEY"));
        assert!(command.contains("--compose-root"));
        assert!(!command.contains("fake-bot-secret-canary"));
        assert!(!command.contains("fake-stt-secret-canary"));
        assert!(!command.contains("--token "));
        assert!(!command.contains("--stt-api-key "));
        let no_voice = launch_command(&spec, Path::new("team-bot"), root.path(), "7", false);
        assert!(!no_voice.contains("--stt-api-key-env"));
    }
}
