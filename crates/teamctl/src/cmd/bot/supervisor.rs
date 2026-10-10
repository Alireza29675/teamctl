use super::*;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

// ── Spawn helpers (used by cmd::up / cmd::down) ─────────────────────

#[derive(serde::Serialize)]
pub struct BotSpec {
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
    out
}

/// Spawn one tmux session running `team-bot` for this manager.
/// No-op if already running. Skips and warns when env vars are unset.
pub fn up_one(spec: &BotSpec, team_bot_bin: &Path, root: &Path) -> Result<bool> {
    let token = match std::env::var(&spec.token_env) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "skip · bot {} ({} unset — run `teamctl bot setup`)",
                spec.session, spec.token_env
            );
            return Ok(false);
        }
    };
    let chats = std::env::var(&spec.chats_env).unwrap_or_default();

    let mut saved = load_sessions(root)?;
    let fallback = legacy_session(spec, root);
    let observed = saved.get(&spec.manager).unwrap_or(&fallback);
    let supervisor = TmuxSupervisor;
    if supervisor.state(&session_spec(spec, root, observed))? == AgentState::Running {
        return Ok(true);
    }
    if observed != &spec.session
        && supervisor.state(&session_spec(spec, root, &spec.session))? == AgentState::Running
    {
        saved.insert(spec.manager.clone(), spec.session.clone());
        save_sessions(root, &saved)?;
        return Ok(true);
    }

    // T-101 voice STT: when `speech_to_text` is configured for this manager,
    // resolve the API key env var here (mirrors the bot-token pattern) and
    // append the STT flags to the spawn command. An unset key downgrades the
    // bot to "no voice" rather than aborting the spawn — text and media keep
    // working.
    let stt_flags = match &spec.stt {
        Some(stt) => match std::env::var(&stt.api_key_env) {
            Ok(v) if !v.is_empty() => {
                let mut s = format!(
                    " --stt-provider {p} --stt-api-key {k} --stt-model {m}",
                    p = shlex_quote(&stt.provider),
                    k = shlex_quote(&v),
                    m = shlex_quote(&stt.model),
                );
                if let Some(lang) = &stt.language {
                    s.push_str(&format!(" --stt-language {l}", l = shlex_quote(lang)));
                }
                s
            }
            _ => {
                eprintln!(
                    "skip voice · bot {} ({} unset — voice messages will be ignored)",
                    spec.session, stt.api_key_env
                );
                String::new()
            }
        },
        None => String::new(),
    };

    // T-367: forward the manager's display_name so the bot's first-connect
    // greeting can read "Connected to <name> via teamctl". Omitted entirely
    // when unset, so the bot falls back to the `<project>:<manager>` id.
    let display_name_flag = match &spec.display_name {
        Some(dn) => format!(" --manager-display-name {}", shlex_quote(dn)),
        None => String::new(),
    };

    let cmd = format!(
        "{bin} --mailbox {mb} --token {tok} --authorized-chat-ids {chats} \
         --manager {mgr} --tmux-prefix {prefix}{dn}{stt}",
        bin = shlex_quote(&team_bot_bin.display().to_string()),
        mb = shlex_quote(&spec.mailbox.display().to_string()),
        tok = shlex_quote(&token),
        chats = shlex_quote(&chats),
        mgr = shlex_quote(&spec.manager),
        prefix = shlex_quote(&spec.tmux_prefix),
        dn = display_name_flag,
        stt = stt_flags,
    );
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
    saved.insert(spec.manager.clone(), spec.session.clone());
    save_sessions(root, &saved)?;
    super::reconcile::record_input(root, spec)?;
    Ok(true)
}

pub fn down_one(spec: &BotSpec, root: &Path) -> Result<()> {
    down_one_with(spec, root, &TmuxSupervisor)
}

pub(super) fn observed_state(spec: &BotSpec, root: &Path) -> Result<(String, AgentState)> {
    let saved = load_sessions(root)?;
    let session = saved
        .get(&spec.manager)
        .cloned()
        .unwrap_or_else(|| legacy_session(spec, root));
    let state = TmuxSupervisor.state(&session_spec(spec, root, &session))?;
    Ok((session, state))
}

fn down_one_with(spec: &BotSpec, root: &Path, supervisor: &impl Supervisor) -> Result<()> {
    let mut saved = load_sessions(root)?;
    let fallback = legacy_session(spec, root);
    let session = saved.get(&spec.manager).unwrap_or(&fallback);
    supervisor.down(&session_spec(spec, root, session))?;
    saved.remove(&spec.manager);
    save_sessions(root, &saved)?;
    super::reconcile::forget_input(root, &spec.manager)
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
                .get(&spec.manager)
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
