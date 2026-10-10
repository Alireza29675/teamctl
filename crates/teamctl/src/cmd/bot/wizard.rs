use super::*;

pub(super) fn setup(root: &Path, force: bool, only_manager: Option<String>) -> Result<()> {
    source_env_files(root);
    let compose = super::super::load(root)?;

    let all_managers = all_managers(&compose);
    if all_managers.is_empty() {
        println!("No managers in compose. Add one to `projects/<id>.yaml` and re-run.");
        return Ok(());
    }

    // No redundant "teamctl bot setup" heading (the operator just typed it).
    // A grey italic note on scope instead.
    println!(
        "{}",
        styled(
            "3;90",
            "Only Telegram is supported in this version — more interfaces are on the way."
        )
    );

    // Fork at the top: managed bots (one manager bot spawns the per-agent
    // child bots) vs manual token (the original BotFather-per-manager
    // walkthrough). Targeting a single manager with `--manager` is a manual
    // operation by nature, so it skips the fork and stays on the manual path.
    match choose_setup_mode(only_manager.is_some())? {
        SetupMode::Manual => manual_setup(root, &compose, force, only_manager, &all_managers),
        SetupMode::Managed => managed_setup(root, &compose, force),
    }
}

/// Which setup path the operator picked at the top-level fork.
pub(super) enum SetupMode {
    /// Original path: operator pastes a BotFather token per manager.
    Manual,
    /// New path: one manager bot programmatically spawns per-agent child
    /// bots (Telegram Managed Bots, Bot API 9.6).
    Managed,
}

/// Present the managed-vs-manual fork. `forced_manual` short-circuits to
/// the manual path (used when `--manager` targets a single manager, which
/// the managed whole-project flow doesn't model).
pub(super) fn choose_setup_mode(forced_manual: bool) -> Result<SetupMode> {
    if forced_manual {
        return Ok(SetupMode::Manual);
    }
    println!("\nHow do you want to set up Telegram bots?");
    println!(
        "  1) Manual token — paste a BotFather token for each manager {}",
        styled("90", "(the original flow)")
    );
    println!(
        "  2) Managed bots — one manager bot spawns a child bot per agent {}",
        styled(
            "90",
            "(needs a manager bot with Managed Bots enabled; the Telegram-side bot creation is rougher)"
        )
    );
    loop {
        // Empty input (just Enter) defaults to 1, the manual flow.
        match prompt("Choose [1/2] (default 1): ")?.trim() {
            "1" | "" => return Ok(SetupMode::Manual),
            "2" => return Ok(SetupMode::Managed),
            other => println!("  `{other}` — please enter 1 or 2."),
        }
    }
}

/// The original per-manager BotFather walkthrough, unchanged. Every manager
/// (or the single `--manager` target) is walked through `wizard_one`.
pub(super) fn manual_setup(
    root: &Path,
    compose: &Compose,
    force: bool,
    only_manager: Option<String>,
    all_managers: &[String],
) -> Result<()> {
    let filtered: Vec<String> = match only_manager.as_deref() {
        Some(m) => {
            if !all_managers.contains(&m.to_string()) {
                bail!(
                    "manager `{m}` not found. Known: {}",
                    all_managers.join(", ")
                );
            }
            vec![m.to_string()]
        }
        None => all_managers.to_vec(),
    };

    let mut configured = 0usize;
    let mut skipped = 0usize;
    for mgr in &filtered {
        match wizard_one(root, compose, mgr, force)? {
            WizardOutcome::Configured => configured += 1,
            WizardOutcome::AlreadyConfigured => skipped += 1,
            WizardOutcome::Cancelled => {}
        }
    }

    println!();
    println!(
        "Done. {configured} configured, {skipped} already set up.\n\
         Run `teamctl up` to launch the bots, then DM each one in Telegram."
    );
    Ok(())
}

// ── Managed-bots path ───────────────────────────────────────────────

/// Conventional env-var name for the project's manager bot token. Mirrors
/// the `TEAMCTL_TG_<ROLE>_TOKEN` per-manager pattern, one level up.
pub(super) fn default_manager_bot_token_env() -> String {
    "TEAMCTL_TG_MANAGER_TOKEN".to_string()
}

/// Pick which project the managed-bots setup targets. A single-project
/// compose (the common case) is chosen automatically; otherwise prompt.
pub(super) fn choose_managed_project(compose: &Compose) -> Result<String> {
    let ids: Vec<String> = compose
        .projects
        .iter()
        .map(|p| p.project.id.clone())
        .collect();
    match ids.as_slice() {
        [] => bail!("no projects in compose"),
        [only] => Ok(only.clone()),
        _ => {
            println!("\nWhich project? {}", ids.join(", "));
            loop {
                let pick = prompt("Project id: ")?.trim().to_string();
                if ids.contains(&pick) {
                    return Ok(pick);
                }
                println!("  `{pick}` not found. Known: {}", ids.join(", "));
            }
        }
    }
}

/// The managed-bots path: configure one manager bot for the project, which
/// then spawns a child bot per manager. v1 collects + validates the manager
/// bot token, writes the `interfaces.telegram.manager_bot` block into the
/// project YAML and the token into `.env`. The per-manager child-bot spawn
/// (emit `t.me/newbot` link → poll → `getManagedBotToken` → write child
/// token) composes on top of the #342 managed-bot client.
pub(super) fn managed_setup(root: &Path, compose: &Compose, force: bool) -> Result<()> {
    let project_id = choose_managed_project(compose)?;

    // Reuse the project's declared manager_bot token_env if present,
    // otherwise the conventional default.
    let token_env = compose
        .projects
        .iter()
        .find(|p| p.project.id == project_id)
        .and_then(|p| p.telegram())
        .and_then(|t| t.manager_bot.as_ref())
        .map(|m| m.token_env.clone())
        .unwrap_or_else(default_manager_bot_token_env);

    let token_set = trimmed_env(&token_env).is_some();

    println!("\n── managed bots · project `{project_id}` ──");
    let token = if force || !token_set {
        println!(
            "\nStep — Create your manager bot.\n\
               Open https://t.me/BotFather, send /newbot, then enable Managed Bots\n\
               on it (/mybots → your bot → Bot Settings → Managed Bots).\n\
               BotFather replies with a token like `123456:AAH-…`."
        );
        let t = prompt_secret("Paste manager bot token: ")?
            .trim()
            .to_string();
        if t.is_empty() || !t.contains(':') {
            bail!("invalid token (expected `<id>:<secret>` shape)");
        }
        t
    } else {
        println!("\nUsing existing manager bot token from {token_env}.");
        trimmed_env(&token_env).unwrap()
    };

    println!("Verifying with Telegram…");
    let me = telegram_get_me(&token)?;
    let mgr_username = me.username.as_deref().unwrap_or("your-manager-bot");
    println!(
        "  ✓ @{mgr_username} ({})",
        me.first_name.as_deref().unwrap_or("?")
    );

    upsert_env_var(root, &token_env, &token)?;
    upsert_project_manager_bot(compose, &project_id, &token_env)?;
    println!(
        "  ✓ wrote {token_env} into .team/.env\n\
         \x20\x20✓ interfaces.telegram.manager_bot on project `{project_id}` is up to date"
    );

    // ── Flow A: spawn a child bot per manager via the manager bot ──────
    // For each manager we emit a bot-creation link the operator confirms
    // in Telegram; the manager bot mints a child bot, we pull its token,
    // then run the same `/start` chat-authorization step as the manual
    // flow so each child reaches the identical end-state (token + chat id).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime for managed bots")?;
    let client = ManagedBotClient::new(token.clone());
    let roles: Vec<String> = compose
        .projects
        .iter()
        .find(|p| p.project.id == project_id)
        .map(|p| p.managers.keys().cloned().collect())
        .unwrap_or_default();

    let mut minted = 0usize;
    let mut skipped = 0usize;
    for role in roles {
        let mgr = format!("{project_id}:{role}");
        let (child_token_env, child_chats_env) = manager_telegram(compose, &mgr)
            .unwrap_or_else(|| (default_token_env(&mgr), default_chats_env(&mgr)));

        if !force
            && trimmed_env(&child_token_env).is_some()
            && trimmed_env(&child_chats_env).is_some()
        {
            println!("  ✓ {role} — child bot already set up (skipped)");
            skipped += 1;
            continue;
        }

        println!("\n── child bot · {role} ──");
        let suggested = suggested_child_username(&project_id, &role);
        let link = ManagedBotClient::creation_link(mgr_username, &suggested);
        println!(
            "Open this link in Telegram and confirm the new bot:\n  {link}\n\
             (the bot is handed to @{mgr_username}; we pull its token automatically.)"
        );
        let updated = rt.block_on(client.poll_for_managed_bot())?;
        let child_token = rt.block_on(client.get_managed_bot_token(updated.bot.id))?;

        let child_me = telegram_get_me(&child_token)?;
        let child_username = child_me.username.as_deref().unwrap_or("your-bot");
        println!(
            "  ✓ minted @{child_username}\n\
             Step — Authorize your chat: open @{child_username} and send /start."
        );
        let chat_id = poll_for_start(&child_token, Duration::from_secs(120))?.to_string();

        write_env_file(
            root,
            &child_token_env,
            &child_token,
            &child_chats_env,
            &chat_id,
        )?;
        upsert_manager_telegram(compose, &mgr, &child_token_env, &child_chats_env)?;
        println!("  ✓ {role}: wrote {child_token_env} + {child_chats_env}, telegram block updated");
        minted += 1;
    }

    println!();
    println!(
        "Done. {minted} child bot(s) set up, {skipped} already configured, under \
         manager bot @{mgr_username}.\n\
         Run `teamctl up` to launch them, then DM each one in Telegram."
    );
    Ok(())
}

/// Telegram-legal suggested username for a manager's child bot. Telegram
/// requires `[A-Za-z0-9_]`, ending in `bot`; this is only a suggestion the
/// operator can change in the creation flow.
pub(super) fn suggested_child_username(project_id: &str, role: &str) -> String {
    let base: String = format!("{project_id}_{role}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{base}_bot")
}

pub(super) enum WizardOutcome {
    Configured,
    AlreadyConfigured,
    Cancelled,
}

/// Build the confirm-prompt wording for one manager. Keys on whether
/// `.env` actually holds values (`token_set`/`chats_set`), NOT on whether
/// the template merely declares the env-var names — declaring names alone
/// must read as first-time setup, not a re-run (the bug this fixes). The
/// `(true, true)` arm is only reachable under `--force`: the no-force
/// fully-wired case early-returns in `wizard_one`.
pub(super) fn resume_prompt(
    manager: &str,
    token_env: &str,
    chats_env: &str,
    token_set: bool,
    chats_set: bool,
) -> String {
    match (token_set, chats_set) {
        (true, false) => format!(
            "Resume Telegram setup for {manager}? Token already in {token_env}; \
             we'll just collect the chat id. [Y/n] "
        ),
        (false, true) => format!(
            "Resume Telegram setup for {manager}? Chat id already in {chats_env}; \
             we'll just collect the token. [Y/n] "
        ),
        (true, true) => format!(
            "Re-run Telegram setup for {manager}? Existing env-var names will be reused. [Y/n] "
        ),
        (false, false) => format!("Set up Telegram bot for {manager}? [Y/n] "),
    }
}

/// Walk one manager through whatever steps remain. The wizard is
/// **resumable**: if `interfaces.telegram` is already in the YAML we
/// reuse those env-var names; if either env value is already in `.env`
/// we keep it (re-validating the token via `getMe`) and only prompt
/// for what's still missing. The confirm wording keys on whether
/// `.env` actually holds values — declaring the env-var names in the
/// template alone reads as a first-time setup, not a re-run.
/// `--force` re-asks for everything.
pub(super) fn wizard_one(
    root: &Path,
    compose: &Compose,
    manager: &str,
    force: bool,
) -> Result<WizardOutcome> {
    let existing = manager_telegram(compose, manager);
    let (token_env, chats_env, env_names_chosen_by_user) = match &existing {
        Some((t, c)) => (t.clone(), c.clone(), false),
        None => (default_token_env(manager), default_chats_env(manager), true),
    };

    let token_value = trimmed_env(&token_env);
    let chats_value = trimmed_env(&chats_env);
    let token_set = token_value.is_some();
    let chats_set = chats_value.is_some();

    // Fully wired and not forcing: skip silently.
    if !force && existing.is_some() && token_set && chats_set {
        println!("✓ {manager} — already configured (skipped)");
        return Ok(WizardOutcome::AlreadyConfigured);
    }

    println!("\n── {manager} ──");
    let prompt_msg = resume_prompt(manager, &token_env, &chats_env, token_set, chats_set);
    if !confirm(&prompt_msg, true)? {
        println!("  skipped");
        return Ok(WizardOutcome::Cancelled);
    }

    // ── Token: existing one re-validated, otherwise prompt ─────────
    let token = if force || !token_set {
        if force && token_set {
            println!(
                "\nForce re-setup — paste a fresh token from BotFather (existing one in {token_env} will be overwritten):"
            );
        } else {
            println!(
                "\nStep — Create a bot.\n\
                   Open https://t.me/BotFather, send /newbot, follow prompts.\n\
                   BotFather will reply with a token like `123456:AAH-…`."
            );
        }
        let t = prompt_secret("Paste bot token: ")?.trim().to_string();
        if t.is_empty() || !t.contains(':') {
            bail!("invalid token (expected `<id>:<secret>` shape)");
        }
        t
    } else {
        println!("\nUsing existing token from {token_env}.");
        token_value.clone().unwrap()
    };

    println!("Verifying with Telegram…");
    let me = telegram_get_me(&token)?;
    let bot_username = me.username.as_deref().unwrap_or("your-bot");
    println!(
        "  ✓ @{bot_username} ({})",
        me.first_name.as_deref().unwrap_or("?")
    );

    // ── Chat id: existing one trusted, otherwise /start ────────────
    let chat_id = if force || !chats_set {
        println!(
            "\nStep — Authorize your chat.\n\
               Open Telegram, search for @{bot_username}, send /start to it."
        );
        poll_for_start(&token, Duration::from_secs(120))?.to_string()
    } else {
        println!("Using existing chat id(s) from {chats_env}.");
        chats_value.clone().unwrap()
    };

    // ── Env var names: only prompt when the YAML doesn't fix them ──
    let (final_token_env, final_chats_env) = if env_names_chosen_by_user {
        println!("\nStep — Pick env-var names (defaults are fine).");
        let t = prompt_with_default("Token env var", &token_env)?;
        let c = prompt_with_default("Chat-ids env var", &chats_env)?;
        (t, c)
    } else {
        (token_env.clone(), chats_env.clone())
    };

    write_env_file(root, &final_token_env, &token, &final_chats_env, &chat_id)?;
    upsert_manager_telegram(compose, manager, &final_token_env, &final_chats_env)?;

    println!(
        "  ✓ wrote {final_token_env}, {final_chats_env} into .team/.env\n\
         \x20\x20✓ telegram block on manager {manager} in projects/<id>.yaml is up to date"
    );
    Ok(WizardOutcome::Configured)
}

pub(super) fn trimmed_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ── List / status ───────────────────────────────────────────────────
