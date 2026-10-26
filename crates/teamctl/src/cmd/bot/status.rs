use super::*;

pub(super) fn list(root: &Path) -> Result<()> {
    source_env_files(root);
    let compose = super::super::load(root)?;
    let mut any = false;
    println!(
        "{:<24} {:<28} {:<28} {:<8} {:<8}",
        "AGENT / INTERFACE", "TOKEN_ENV", "CHATS_ENV", "TOKEN", "CHATS"
    );
    for spec in bot_specs(&compose) {
        any = true;
        println!(
            "{:<24} {:<28} {:<28} {:<8} {:<8}",
            spec.identity(),
            spec.token_env,
            spec.chats_env,
            env_state(&spec.token_env),
            env_state(&spec.chats_env)
        );
    }
    if !any {
        println!("(no Telegram interfaces configured — try `teamctl bot setup`)");
    }
    Ok(())
}

pub(super) fn status(root: &Path) -> Result<()> {
    let compose = super::super::load(root)?;
    let mut any = false;
    for spec in bot_specs(&compose) {
        any = true;
        let (session, state) = super::supervisor::observed_state(&spec, &compose.root)?;
        println!(
            "{:<24} {:<8} {}",
            spec.identity(),
            if state == team_core::supervisor::AgentState::Running {
                "running"
            } else {
                "stopped"
            },
            session
        );
    }
    if !any {
        println!("(no Telegram interfaces configured — try `teamctl bot setup`)");
    }
    Ok(())
}

pub(super) fn env_state(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => "set".into(),
        _ => "UNSET".into(),
    }
}

// ── Discovery ───────────────────────────────────────────────────────

pub(super) fn all_managers(compose: &Compose) -> Vec<String> {
    let mut out = BTreeSet::new();
    for proj in &compose.projects {
        for role in proj.managers.keys() {
            out.insert(format!("{}:{}", proj.project.id, role));
        }
    }
    out.into_iter().collect()
}

pub(super) fn manager_telegram(compose: &Compose, manager: &str) -> Option<(String, String)> {
    let (project, role) = manager.split_once(':')?;
    let proj = compose.projects.iter().find(|p| p.project.id == project)?;
    let agent = proj.managers.get(role)?;
    let tg = agent.telegram()?;
    Some((tg.bot_token_env.clone(), tg.chat_ids_env.clone()))
}

pub(super) fn default_token_env(manager: &str) -> String {
    let role = manager.split_once(':').map(|(_, r)| r).unwrap_or(manager);
    format!("TEAMCTL_TG_{}_TOKEN", role.to_uppercase().replace('-', "_"))
}

pub(super) fn default_chats_env(manager: &str) -> String {
    let role = manager.split_once(':').map(|(_, r)| r).unwrap_or(manager);
    format!("TEAMCTL_TG_{}_CHATS", role.to_uppercase().replace('-', "_"))
}

/// `<prefix>bot-<project>-<manager>`. Validation rejects overlapping agent
/// session names in this same normalized tmux namespace.
pub fn bot_session_name(tmux_prefix: &str, manager: &str) -> String {
    let safe = manager.replace(':', "-");
    format!("{tmux_prefix}bot-{safe}")
}

// ── Prompts ─────────────────────────────────────────────────────────
