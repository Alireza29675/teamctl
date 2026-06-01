//! `teamctl bot` — set up and supervise 1:1 Telegram bots, one per
//! user-facing manager.
//!
//! `bot setup` walks the operator through BotFather → token → `/start`
//! → chat id, lets them pick env-var names (sensible defaults), writes
//! the values into `.team/.env`, and upserts a `telegram:` block into
//! the manager definition in `projects/<id>.yaml`. After setup,
//! `teamctl up` spawns one `team-bot` per manager-with-`telegram` so
//! the human DMs the manager's bot directly.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use team_core::compose::Compose;

use crate::managed_bot::ManagedBotClient;

pub fn run(root: &Path, action: BotAction) -> Result<()> {
    match action {
        BotAction::Setup { force, manager } => setup(root, force, manager),
        BotAction::List => list(root),
        BotAction::Status => status(root),
    }
}

#[derive(Debug)]
pub enum BotAction {
    Setup {
        force: bool,
        manager: Option<String>,
    },
    List,
    Status,
}

// ── Setup wizard ────────────────────────────────────────────────────

fn setup(root: &Path, force: bool, only_manager: Option<String>) -> Result<()> {
    source_env_files(root);
    let compose = super::load(root)?;

    let all_managers = all_managers(&compose);
    if all_managers.is_empty() {
        println!("No managers in compose. Add one to `projects/<id>.yaml` and re-run.");
        return Ok(());
    }

    println!("teamctl bot setup");
    println!("─────────────────");

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
enum SetupMode {
    /// Original path: operator pastes a BotFather token per manager.
    Manual,
    /// New path: one manager bot programmatically spawns per-agent child
    /// bots (Telegram Managed Bots, Bot API 9.6).
    Managed,
}

/// Present the managed-vs-manual fork. `forced_manual` short-circuits to
/// the manual path (used when `--manager` targets a single manager, which
/// the managed whole-project flow doesn't model).
fn choose_setup_mode(forced_manual: bool) -> Result<SetupMode> {
    if forced_manual {
        return Ok(SetupMode::Manual);
    }
    println!("\nHow do you want to set up Telegram bots?");
    println!("  1) Manual token — paste a BotFather token for each manager (the original flow)");
    println!("  2) Managed bots — one manager bot spawns a child bot per agent (needs a manager bot with Managed Bots enabled; the Telegram-side bot creation is rougher)");
    loop {
        match prompt("Choose [1/2]: ")?.trim() {
            "1" | "" => return Ok(SetupMode::Manual),
            "2" => return Ok(SetupMode::Managed),
            other => println!("  `{other}` — please enter 1 or 2."),
        }
    }
}

/// The original per-manager BotFather walkthrough, unchanged. Every manager
/// (or the single `--manager` target) is walked through `wizard_one`.
fn manual_setup(
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
fn default_manager_bot_token_env() -> String {
    "TEAMCTL_TG_MANAGER_TOKEN".to_string()
}

/// Pick which project the managed-bots setup targets. A single-project
/// compose (the common case) is chosen automatically; otherwise prompt.
fn choose_managed_project(compose: &Compose) -> Result<String> {
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
fn managed_setup(root: &Path, compose: &Compose, force: bool) -> Result<()> {
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
fn suggested_child_username(project_id: &str, role: &str) -> String {
    let base: String = format!("{project_id}_{role}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{base}_bot")
}

/// Upsert a single `KEY=value` line into `.team/.env` (replace in place if
/// present, append otherwise) and mirror it into the live process env.
fn upsert_env_var(root: &Path, key: &str, value: &str) -> Result<()> {
    let path = root.join(".env");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut wrote = false;
    for line in existing.lines() {
        let trimmed = line.trim_start();
        let k = trimmed
            .strip_prefix("export ")
            .unwrap_or(trimmed)
            .split_once('=')
            .map(|(k, _)| k.trim());
        if k == Some(key) {
            out.push_str(&format!("{key}={value}\n"));
            wrote = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !wrote {
        out.push_str(&format!("{key}={value}\n"));
    }
    fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
    // SAFETY: single-threaded CLI startup.
    unsafe {
        std::env::set_var(key, value);
    }
    Ok(())
}

/// Write `interfaces.telegram.manager_bot.token_env` into the project's YAML
/// via the comment-preserving `yaml_edit::save` path. Seeds a top-level
/// `interfaces:` key first when absent — `set_nested_mapping` can only
/// splice beneath an existing top-level key.
fn upsert_project_manager_bot(compose: &Compose, project_id: &str, token_env: &str) -> Result<()> {
    // Same global.projects[] ↔ projects[] ordering invariant the manual
    // path relies on (see `upsert_manager_telegram`).
    let proj_ref = compose
        .global
        .projects
        .iter()
        .zip(compose.projects.iter())
        .find(|(_, p)| p.project.id == project_id)
        .map(|(r, _)| r)
        .ok_or_else(|| anyhow!("project `{project_id}` not found in compose"))?;
    let path = compose.root.join(&proj_ref.file);

    let doc = team_core::yaml_edit::load(&path)?;
    let doc = splice_project_manager_bot(doc, token_env)?;
    team_core::yaml_edit::save(&doc, &path)?;
    Ok(())
}

/// Pure doc transform behind [`upsert_project_manager_bot`]: ensure a
/// top-level `interfaces:` mapping exists (seed it when absent, since
/// `set_nested_mapping` only splices beneath an existing top-level key),
/// then splice `interfaces.telegram.manager_bot.token_env`. Kept separate
/// so the comment-preservation contract is unit-testable without a Compose.
fn splice_project_manager_bot(
    doc: team_core::yaml_edit::Document,
    token_env: &str,
) -> Result<team_core::yaml_edit::Document> {
    let has_interfaces = doc
        .as_mapping()
        .and_then(|m| m.get_mapping("interfaces"))
        .is_some();
    let doc = if has_interfaces {
        doc
    } else {
        let mut source = doc.to_string();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        source.push_str("interfaces:\n");
        source
            .parse()
            .context("re-parse YAML after seeding `interfaces:`")?
    };

    team_core::yaml_edit::set_nested_mapping(
        doc,
        &["interfaces", "telegram", "manager_bot"],
        &[("token_env", token_env)],
    )
}

enum WizardOutcome {
    Configured,
    AlreadyConfigured,
    Cancelled,
}

/// Walk one manager through whatever steps remain. The wizard is
/// **resumable**: if `interfaces.telegram` is already in the YAML we
/// reuse those env-var names; if either env value is already in `.env`
/// we keep it (re-validating the token via `getMe`) and only prompt
/// for what's still missing. `--force` re-asks for everything.
fn wizard_one(root: &Path, compose: &Compose, manager: &str, force: bool) -> Result<WizardOutcome> {
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
    let prompt_msg = match (existing.is_some(), token_set, chats_set) {
        (true, true, false) => format!(
            "Resume Telegram setup for {manager}? Token already in {token_env}; \
             we'll just collect the chat id. [Y/n] "
        ),
        (true, false, true) => format!(
            "Resume Telegram setup for {manager}? Chat id already in {chats_env}; \
             we'll just collect the token. [Y/n] "
        ),
        (true, _, _) => format!(
            "Re-run Telegram setup for {manager}? Existing env-var names will be reused. [Y/n] "
        ),
        _ => format!("Set up Telegram bot for {manager}? [Y/n] "),
    };
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

fn trimmed_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ── List / status ───────────────────────────────────────────────────

fn list(root: &Path) -> Result<()> {
    source_env_files(root);
    let compose = super::load(root)?;
    let mut any = false;
    println!(
        "{:<24} {:<28} {:<28} {:<8} {:<8}",
        "MANAGER", "TOKEN_ENV", "CHATS_ENV", "TOKEN", "CHATS"
    );
    for proj in &compose.projects {
        for (role, agent) in &proj.managers {
            if let Some(tg) = agent.telegram() {
                any = true;
                let mgr = format!("{}:{}", proj.project.id, role);
                println!(
                    "{:<24} {:<28} {:<28} {:<8} {:<8}",
                    mgr,
                    tg.bot_token_env,
                    tg.chat_ids_env,
                    env_state(&tg.bot_token_env),
                    env_state(&tg.chat_ids_env),
                );
            }
        }
    }
    if !any {
        println!("(no managers have an `interfaces.telegram` block — try `teamctl bot setup`)");
    }
    Ok(())
}

fn status(root: &Path) -> Result<()> {
    let compose = super::load(root)?;
    let prefix = &compose.global.supervisor.tmux_prefix;
    let mut any = false;
    for proj in &compose.projects {
        for (role, agent) in &proj.managers {
            if agent.telegram().is_some() {
                any = true;
                let mgr = format!("{}:{}", proj.project.id, role);
                let session = bot_session_name(prefix, &mgr);
                let running = Command::new("tmux")
                    .args(["has-session", "-t", &session])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                println!(
                    "{:<24} {:<8} {}",
                    mgr,
                    if running { "running" } else { "stopped" },
                    session
                );
            }
        }
    }
    if !any {
        println!("(no managers have an `interfaces.telegram` block — try `teamctl bot setup`)");
    }
    Ok(())
}

fn env_state(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => "set".into(),
        _ => "UNSET".into(),
    }
}

// ── Discovery ───────────────────────────────────────────────────────

fn all_managers(compose: &Compose) -> Vec<String> {
    let mut out = BTreeSet::new();
    for proj in &compose.projects {
        for role in proj.managers.keys() {
            out.insert(format!("{}:{}", proj.project.id, role));
        }
    }
    out.into_iter().collect()
}

fn manager_telegram(compose: &Compose, manager: &str) -> Option<(String, String)> {
    let (project, role) = manager.split_once(':')?;
    let proj = compose.projects.iter().find(|p| p.project.id == project)?;
    let agent = proj.managers.get(role)?;
    let tg = agent.telegram()?;
    Some((tg.bot_token_env.clone(), tg.chat_ids_env.clone()))
}

fn default_token_env(manager: &str) -> String {
    let role = manager.split_once(':').map(|(_, r)| r).unwrap_or(manager);
    format!("TEAMCTL_TG_{}_TOKEN", role.to_uppercase().replace('-', "_"))
}

fn default_chats_env(manager: &str) -> String {
    let role = manager.split_once(':').map(|(_, r)| r).unwrap_or(manager);
    format!("TEAMCTL_TG_{}_CHATS", role.to_uppercase().replace('-', "_"))
}

/// `<prefix>bot-<project>-<manager>` — keeps it unique across projects
/// without colliding with agent-session names (`<prefix><project>-<agent>`).
pub fn bot_session_name(tmux_prefix: &str, manager: &str) -> String {
    let safe = manager.replace(':', "-");
    format!("{tmux_prefix}bot-{safe}")
}

// ── Prompts ─────────────────────────────────────────────────────────

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read stdin")?;
    Ok(line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string())
}

/// Read one line and strip the trailing newline (and a preceding CR),
/// matching [`prompt`]'s capture behaviour exactly so swapping a field
/// from `prompt` to `prompt_secret` changes only the echo, never the
/// captured value. Split out so the value-correctness contract is
/// unit-testable without a terminal.
fn capture_line<R: BufRead>(mut reader: R) -> io::Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string())
}

/// Prompt for a secret (the Telegram bot token). On an interactive
/// unix terminal the input is read with echo disabled — typed *and*
/// pasted characters never appear in the terminal, scrollback, a
/// screen-share, or a recording (T-314). The captured value is
/// unaffected: echo is a display concern only.
///
/// Non-interactive stdin (pipe/redirect — tests, automation) has no
/// terminal echo to suppress and `tcgetattr` would fail on a non-tty,
/// so it falls back to a plain read. Non-unix also falls back; the CI
/// matrix and supported install targets are POSIX, where the masking
/// is effective.
fn prompt_secret(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().ok();

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let fd = io::stdin().as_raw_fd();
        // SAFETY: `isatty` on any fd is defined and side-effect-free.
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        if is_tty {
            // Restore the original terminal attributes on every exit
            // path (normal return, `?` early-return, panic) so a
            // failure can't strand the terminal with echo disabled.
            struct RestoreEcho {
                fd: i32,
                original: libc::termios,
            }
            impl Drop for RestoreEcho {
                fn drop(&mut self) {
                    // SAFETY: `original` was filled by a successful
                    // `tcgetattr` on this same fd.
                    unsafe {
                        libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
                    }
                }
            }

            // SAFETY: `termios` is a C POD; `tcgetattr` fully
            // initialises it for a valid terminal fd.
            let mut term: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut term) } != 0 {
                return Err(io::Error::last_os_error()).context("tcgetattr (mask token input)");
            }
            let _restore = RestoreEcho { fd, original: term };
            // Clear ECHO only — keep ICANON (line editing + Enter) and
            // ISIG (Ctrl-C) so it behaves like a normal password
            // prompt, just silent.
            term.c_lflag &= !libc::ECHO;
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) } != 0 {
                return Err(io::Error::last_os_error())
                    .context("tcsetattr disable echo (mask token input)");
            }

            let value = capture_line(io::stdin().lock()).context("read stdin")?;
            // The user's Enter wasn't echoed — advance the line so the
            // next output doesn't run onto the prompt text.
            println!();
            return Ok(value);
        }
    }

    capture_line(io::stdin().lock()).context("read stdin")
}

fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    let raw = prompt(&format!("{label} [{default}]: "))?;
    let raw = raw.trim();
    Ok(if raw.is_empty() {
        default.to_string()
    } else {
        raw.to_string()
    })
}

fn confirm(msg: &str, default_yes: bool) -> Result<bool> {
    let raw = prompt(msg)?.trim().to_lowercase();
    if raw.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(raw.as_str(), "y" | "yes"))
}

// ── .env file write ────────────────────────────────────────────────

fn source_env_files(root: &Path) {
    for f in [
        root.join(".env"),
        root.parent().unwrap_or(root).join(".env"),
    ] {
        if f.is_file() {
            if let Ok(raw) = fs::read_to_string(&f) {
                for line in raw.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let line = line.strip_prefix("export ").unwrap_or(line);
                    if let Some((k, v)) = line.split_once('=') {
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var_os(k).is_none() {
                            // SAFETY: single-threaded CLI startup.
                            unsafe { std::env::set_var(k, v) };
                        }
                    }
                }
            }
        }
    }
}

fn write_env_file(root: &Path, k1: &str, v1: &str, k2: &str, v2: &str) -> Result<()> {
    let path = root.join(".env");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut wrote_k1 = false;
    let mut wrote_k2 = false;
    for line in existing.lines() {
        let trimmed = line.trim_start();
        let key = trimmed
            .strip_prefix("export ")
            .unwrap_or(trimmed)
            .split_once('=')
            .map(|(k, _)| k.trim());
        match key {
            Some(k) if k == k1 => {
                out.push_str(&format!("{k1}={v1}\n"));
                wrote_k1 = true;
            }
            Some(k) if k == k2 => {
                out.push_str(&format!("{k2}={v2}\n"));
                wrote_k2 = true;
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !wrote_k1 {
        out.push_str(&format!("{k1}={v1}\n"));
    }
    if !wrote_k2 {
        out.push_str(&format!("{k2}={v2}\n"));
    }
    fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
    // SAFETY: single-threaded CLI startup.
    unsafe {
        std::env::set_var(k1, v1);
        std::env::set_var(k2, v2);
    }
    Ok(())
}

// ── projects/<id>.yaml: upsert telegram block on a manager ──────────

fn upsert_manager_telegram(
    compose: &Compose,
    manager: &str,
    token_env: &str,
    chats_env: &str,
) -> Result<()> {
    let (project_id, role) = manager
        .split_once(':')
        .ok_or_else(|| anyhow!("manager must be `<project>:<role>`"))?;

    // Locate the project file path via global.projects[].file. Use the
    // documented `compose.global.projects[i] ↔ compose.projects[i]`
    // ordering invariant (preserved by `Compose::load`) to read
    // `project.id` from the parsed in-memory state rather than
    // re-reading each candidate file from disk. T-238: the old disk
    // re-read silently failed on a file we'd just written through
    // `team_core::yaml_edit::save` in a previous loop iteration
    // (serde_yaml's strict parser disagreeing with yaml_edit's
    // roundtrip on YAML quirks), producing a misleading "project not
    // found" on the second manager when both lived in the same file.
    let proj_ref = compose
        .global
        .projects
        .iter()
        .zip(compose.projects.iter())
        .find(|(_, p)| p.project.id == project_id)
        .map(|(r, _)| r)
        .ok_or_else(|| anyhow!("project `{project_id}` not found in compose"))?;

    let path = compose.root.join(&proj_ref.file);
    edit_manager_yaml(&path, role, token_env, chats_env)
}

/// Rewrites managers.<role>.interfaces.telegram with the new env-var
/// names. Other interface adapters under `interfaces:` (e.g. `discord:`)
/// are preserved, as are comments and blank-line clusters elsewhere in
/// the file (via `team_core::yaml_edit`'s comment-preserving substrate).
fn edit_manager_yaml(path: &Path, role: &str, token_env: &str, chats_env: &str) -> Result<()> {
    let doc = team_core::yaml_edit::load(path)?;

    // Sanity-check that the parent path exists before we splice. Errors
    // here match the pre-substrate behaviour callers rely on.
    let root = doc
        .as_mapping()
        .ok_or_else(|| anyhow!("root of {} is not a mapping", path.display()))?;
    let managers = root
        .get_mapping("managers")
        .ok_or_else(|| anyhow!("`managers:` block missing in {}", path.display()))?;
    if managers.get_mapping(role).is_none() {
        return Err(anyhow!("manager `{role}` missing in {}", path.display()));
    }

    let doc = team_core::yaml_edit::set_nested_mapping(
        doc,
        &["managers", role, "interfaces", "telegram"],
        &[("bot_token_env", token_env), ("chat_ids_env", chats_env)],
    )?;
    team_core::yaml_edit::save(&doc, path)?;
    Ok(())
}

// ── Telegram HTTP via curl ──────────────────────────────────────────

#[derive(Debug)]
struct TelegramUser {
    username: Option<String>,
    first_name: Option<String>,
}

fn telegram_get_me(token: &str) -> Result<TelegramUser> {
    let url = format!("https://api.telegram.org/bot{token}/getMe");
    let body = curl_get(&url)?;
    let v: serde_json::Value = serde_json::from_str(&body).context("parse getMe response")?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        let desc = v
            .get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("(no description)");
        bail!("Telegram rejected token: {desc}");
    }
    let r = v
        .get("result")
        .ok_or_else(|| anyhow!("getMe: no `result`"))?;
    Ok(TelegramUser {
        username: r
            .get("username")
            .and_then(|x| x.as_str())
            .map(str::to_owned),
        first_name: r
            .get("first_name")
            .and_then(|x| x.as_str())
            .map(str::to_owned),
    })
}

fn poll_for_start(token: &str, deadline: Duration) -> Result<i64> {
    let started = Instant::now();
    let mut offset: i64 = 0;
    print!("  waiting for /start ");
    io::stdout().flush().ok();
    while started.elapsed() < deadline {
        let url =
            format!("https://api.telegram.org/bot{token}/getUpdates?timeout=10&offset={offset}");
        let body = match curl_get(&url) {
            Ok(b) => b,
            Err(_) => {
                print!(".");
                io::stdout().flush().ok();
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let v: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            print!(".");
            io::stdout().flush().ok();
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let updates = v
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        for u in &updates {
            if let Some(uid) = u.get("update_id").and_then(|x| x.as_i64()) {
                offset = offset.max(uid + 1);
            }
            let text = u
                .get("message")
                .and_then(|m| m.get("text"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if text.trim_start().starts_with("/start") {
                if let Some(cid) = u
                    .get("message")
                    .and_then(|m| m.get("chat"))
                    .and_then(|c| c.get("id"))
                    .and_then(|x| x.as_i64())
                {
                    println!();
                    return Ok(cid);
                }
            }
        }
        print!(".");
        io::stdout().flush().ok();
    }
    println!();
    bail!("timed out waiting for /start (2 minutes)")
}

fn curl_get(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "15", url])
        .output()
        .context("run curl (is curl installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("curl failed: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── Spawn helpers (used by cmd::up / cmd::down) ─────────────────────

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

    let already = Command::new("tmux")
        .args(["has-session", "-t", &spec.session])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if already {
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
    Ok(true)
}

pub fn down_one(spec: &BotSpec) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &spec.session])
        .status();
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

fn shlex_quote(s: &str) -> String {
    shlex::try_quote(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| format!("'{}'", s.replace('\'', "'\\''")))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_token_env_strips_project() {
        assert_eq!(
            default_token_env("teamctl:eng_lead"),
            "TEAMCTL_TG_ENG_LEAD_TOKEN"
        );
        assert_eq!(default_token_env("startup:pm"), "TEAMCTL_TG_PM_TOKEN");
    }

    #[test]
    fn default_chats_env_matches_token_shape() {
        assert_eq!(default_chats_env("p:role-x"), "TEAMCTL_TG_ROLE_X_CHATS");
    }

    // T-314: token-input masking is display-only — the captured value
    // must be byte-identical to what was entered (and identical to
    // `prompt`'s capture, so the prompt→prompt_secret swap changes
    // only the echo). The echo-off termios path needs a real tty and
    // can't be unit-tested; `capture_line` is the value-correctness
    // contract that can.
    #[test]
    fn capture_line_strips_only_the_trailing_newline() {
        assert_eq!(
            capture_line(&b"123456:AAH-abcDEF\n"[..]).unwrap(),
            "123456:AAH-abcDEF"
        );
    }

    #[test]
    fn capture_line_strips_crlf() {
        assert_eq!(
            capture_line(&b"123:tok-_x.Y\r\n"[..]).unwrap(),
            "123:tok-_x.Y"
        );
    }

    #[test]
    fn capture_line_preserves_value_bytes_including_inner_spaces() {
        // Only the line terminator is stripped; inner/edge spaces and
        // token punctuation survive verbatim (the caller applies its
        // own `.trim()` + shape check — capture must not pre-mangle).
        assert_eq!(
            capture_line(&b"  12:AA b:c  \n"[..]).unwrap(),
            "  12:AA b:c  "
        );
    }

    #[test]
    fn capture_line_handles_eof_without_newline() {
        assert_eq!(
            capture_line(&b"123:no-newline"[..]).unwrap(),
            "123:no-newline"
        );
    }

    #[test]
    fn capture_line_matches_prompt_capture_semantics() {
        // Pin parity with `prompt`'s exact trim chain so the swapped
        // field behaves identically on capture.
        let raw = "999:Zz-_.\r\n";
        let via_capture = capture_line(raw.as_bytes()).unwrap();
        let via_prompt_logic = raw
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        assert_eq!(via_capture, via_prompt_logic);
        assert_eq!(via_capture, "999:Zz-_.");
    }

    #[test]
    fn bot_session_name_is_stable_and_unique() {
        assert_eq!(bot_session_name("t-", "teamctl:pm"), "t-bot-teamctl-pm");
        assert_eq!(
            bot_session_name("a-", "startup:eng_lead"),
            "a-bot-startup-eng_lead"
        );
    }

    #[test]
    fn write_env_file_replaces_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        std::fs::write(
            &env_path,
            "EXISTING=value\nTEAMCTL_TG_PM_TOKEN=oldtok\nKEEP=me\n",
        )
        .unwrap();
        write_env_file(
            dir.path(),
            "TEAMCTL_TG_PM_TOKEN",
            "newtok",
            "TEAMCTL_TG_PM_CHATS",
            "12345",
        )
        .unwrap();
        let got = std::fs::read_to_string(&env_path).unwrap();
        assert!(got.contains("EXISTING=value"));
        assert!(got.contains("KEEP=me"));
        assert!(got.contains("TEAMCTL_TG_PM_TOKEN=newtok"));
        assert!(!got.contains("oldtok"));
        assert!(got.contains("TEAMCTL_TG_PM_CHATS=12345"));
    }

    #[test]
    fn upsert_manager_telegram_succeeds_for_consecutive_managers_in_same_project() {
        // T-238 regression: `teamctl bot setup` walks every manager in
        // the compose, and `upsert_manager_telegram` writes the
        // telegram block per manager via `team_core::yaml_edit::save`.
        // The pre-fix project-lookup re-read each candidate file from
        // disk with `serde_yaml::from_str` to match by `project.id`;
        // after the first iteration's write through yaml_edit, a
        // strict re-read of the same file could silently fail
        // (`.unwrap_or(false)` in the find-closure), so the second
        // manager's lookup hit no match and bailed with "project not
        // found in compose" even though both managers lived in the
        // file the loop had just edited.
        //
        // Pins the second-iteration shape: build a minimal `.team/`
        // with two managers in one project, load the compose, call
        // upsert for each in sequence, and assert both calls succeed
        // AND the file ends with both telegram blocks.
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join(".team");
        std::fs::create_dir_all(team.join("projects")).unwrap();
        std::fs::create_dir_all(team.join("roles")).unwrap();
        std::fs::write(
            team.join("team-compose.yaml"),
            "version: 2\n\
             broker:\n  type: sqlite\n  path: state/mailbox.db\n\
             supervisor:\n  type: tmux\n  tmux_prefix: a-\n\
             projects:\n  - file: projects/p.yaml\n",
        )
        .unwrap();
        std::fs::write(
            team.join("projects/p.yaml"),
            "version: 2\n\
             project:\n  id: p\n  name: P\n  cwd: .\n\
             managers:\n\
             \x20\x20alpha:\n    runtime: claude-code\n    role_prompt: roles/alpha.md\n\
             \x20\x20beta:\n    runtime: claude-code\n    role_prompt: roles/beta.md\n",
        )
        .unwrap();
        std::fs::write(team.join("roles/alpha.md"), "alpha\n").unwrap();
        std::fs::write(team.join("roles/beta.md"), "beta\n").unwrap();

        let compose = Compose::load(&team).expect("compose loads cleanly");

        upsert_manager_telegram(&compose, "p:alpha", "ALPHA_TOKEN", "ALPHA_CHATS")
            .expect("first manager upsert succeeds");

        upsert_manager_telegram(&compose, "p:beta", "BETA_TOKEN", "BETA_CHATS")
            .expect("second manager upsert in same project must succeed");

        let got = std::fs::read_to_string(team.join("projects/p.yaml")).unwrap();
        assert!(
            got.contains("ALPHA_TOKEN") && got.contains("ALPHA_CHATS"),
            "first manager's telegram block must survive the second write:\n{got}"
        );
        assert!(
            got.contains("BETA_TOKEN") && got.contains("BETA_CHATS"),
            "second manager's telegram block must land:\n{got}"
        );
    }

    #[test]
    fn fresh_essentials_team_bot_setup_yields_resolvable_bridge_spec() {
        // #311 repro. Materialize the REAL shipped `essentials` scaffold
        // (main + ops; `ops:ops` pre-wired with the
        // TEAMCTL_TG_OPS_{TOKEN,CHATS} env-var names), reproduce the
        // two persistence side-effects of `bot setup`'s `wizard_one`
        // for ops:ops (write_env_file + upsert_manager_telegram),
        // reload the compose exactly as `teamctl up` does, and assert
        // the Telegram bridge would receive a usable, resolvable spec.
        //
        // Pins #311's acceptance contract ("fresh init -> bot setup ->
        // round-trip delivers") at the deterministically-testable
        // layer: the persisted config the bridge consumes. The
        // interactive wizard (stdin), Telegram HTTP, and the tmux spawn
        // are out of unit scope — what decides whether the bridge can
        // deliver is exactly the spec + .env this asserts.
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join(".team");

        let ess = crate::cmd::init::TEMPLATES
            .iter()
            .find(|t| t.key == "essentials")
            .expect("essentials template present");
        for (rel, content) in ess.files {
            let body = content
                .replace("{{project_id}}", "main")
                .replace("{{project_name}}", "Main");
            let path = team.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }

        // A strict-parse regression on the shipped essentials tree
        // would itself be #311.
        let compose =
            Compose::load(&team).expect("freshly-scaffolded essentials compose must load");

        assert_eq!(
            all_managers(&compose),
            vec!["ops:ops".to_string()],
            "essentials must expose exactly ops:ops to `bot setup`"
        );

        // The scaffold pre-wires the env-var names; `bot setup` reuses
        // them verbatim (env_names_chosen_by_user == false path).
        let (tok_env, chats_env) = manager_telegram(&compose, "ops:ops")
            .expect("essentials pre-wires ops:ops's telegram env-var names");
        assert_eq!(tok_env, "TEAMCTL_TG_OPS_TOKEN");
        assert_eq!(chats_env, "TEAMCTL_TG_OPS_CHATS");

        write_env_file(&team, &tok_env, "123456:FAKE-TOKEN", &chats_env, "99001122").unwrap();
        upsert_manager_telegram(&compose, "ops:ops", &tok_env, &chats_env).unwrap();

        // Reload exactly as `teamctl up` does, then build bridge specs.
        let compose = Compose::load(&team).expect("compose reloads after bot setup");
        let specs = bot_specs(&compose);
        assert_eq!(specs.len(), 1, "exactly one bot spec (ops:ops) expected");
        let spec = &specs[0];
        assert_eq!(spec.manager, "ops:ops");
        assert_eq!(spec.token_env, "TEAMCTL_TG_OPS_TOKEN");
        assert_eq!(spec.chats_env, "TEAMCTL_TG_OPS_CHATS");

        // up_one() resolves spec.token_env from the sourced .team/.env.
        // File-based assertion (parallel-safe, matching
        // write_env_file_replaces_in_place).
        let env_body = std::fs::read_to_string(team.join(".env")).unwrap();
        assert!(
            env_body.contains("TEAMCTL_TG_OPS_TOKEN=123456:FAKE-TOKEN"),
            "the bot token the bridge resolves must be persisted to .team/.env:\n{env_body}"
        );
        assert!(
            env_body.contains("TEAMCTL_TG_OPS_CHATS=99001122"),
            "the authorized chat id must be persisted to .team/.env:\n{env_body}"
        );

        // The telegram block must round-trip through the typed schema
        // the bridge reads (agent.telegram()) after the yaml_edit write.
        let ops_yaml = std::fs::read_to_string(team.join("projects/ops.yaml")).unwrap();
        let parsed: team_core::compose::Project = serde_yaml::from_str(&ops_yaml).unwrap();
        let tg = parsed
            .managers
            .get("ops")
            .and_then(|a| a.telegram())
            .expect("ops:ops telegram must survive the upsert and re-parse");
        assert_eq!(tg.bot_token_env, "TEAMCTL_TG_OPS_TOKEN");
        assert_eq!(tg.chat_ids_env, "TEAMCTL_TG_OPS_CHATS");
    }

    #[test]
    fn edit_manager_yaml_inserts_interfaces_telegram_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.yaml");
        std::fs::write(
            &path,
            "version: 2\n\
             project:\n  id: p\n  name: P\n  cwd: ..\n\
             managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n",
        )
        .unwrap();
        edit_manager_yaml(&path, "pm", "PM_TOKEN", "PM_CHATS").unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(
            got.contains("interfaces:"),
            "missing interfaces block:\n{got}"
        );
        assert!(got.contains("telegram:"));
        assert!(got.contains("bot_token_env: PM_TOKEN"));
        assert!(got.contains("chat_ids_env: PM_CHATS"));

        // Round-trip: parsing should give us the typed struct.
        let parsed: team_core::compose::Project = serde_yaml::from_str(&got).unwrap();
        let tg = parsed
            .managers
            .get("pm")
            .and_then(|a| a.telegram())
            .expect("telegram parses out");
        assert_eq!(tg.bot_token_env, "PM_TOKEN");

        // Idempotent: re-running replaces telegram, doesn't duplicate.
        edit_manager_yaml(&path, "pm", "PM_TOKEN_2", "PM_CHATS_2").unwrap();
        let got2 = std::fs::read_to_string(&path).unwrap();
        assert_eq!(got2.matches("telegram:").count(), 1);
        assert_eq!(got2.matches("interfaces:").count(), 1);
        assert!(got2.contains("PM_TOKEN_2"));
        assert!(!got2.contains("PM_TOKEN\n"));
    }

    // ── #344 managed-bots: fork + project manager_bot write ──────────

    #[test]
    fn forced_manual_skips_the_fork() {
        // `--manager <m>` (forced_manual=true) must stay on the manual
        // path without prompting, since managed setup is whole-project.
        assert!(matches!(
            choose_setup_mode(true).unwrap(),
            SetupMode::Manual
        ));
    }

    fn manager_bot_token_env(yaml: &str) -> String {
        let p: team_core::compose::Project = serde_yaml::from_str(yaml).unwrap();
        p.telegram()
            .and_then(|t| t.manager_bot.as_ref())
            .expect("manager_bot parses out")
            .token_env
            .clone()
    }

    #[test]
    fn splice_manager_bot_seeds_interfaces_when_absent() {
        // Fresh project YAML (no `interfaces:` block) — the splice must
        // seed the top-level key and write the manager_bot token_env so
        // `Project::telegram()` reads it back.
        let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n";
        let doc: team_core::yaml_edit::Document = src.parse().unwrap();
        let got = splice_project_manager_bot(doc, "TEAMCTL_TG_MANAGER_TOKEN")
            .unwrap()
            .to_string();
        assert!(got.contains("interfaces:"), "missing interfaces:\n{got}");
        assert!(got.contains("manager_bot:"), "missing manager_bot:\n{got}");
        assert_eq!(manager_bot_token_env(&got), "TEAMCTL_TG_MANAGER_TOKEN");
        // Seeding must not duplicate the top-level key.
        assert_eq!(
            got.matches("interfaces:").count(),
            1,
            "dup interfaces:\n{got}"
        );
    }

    #[test]
    fn splice_manager_bot_preserves_trailing_comment_on_insert() {
        // #319 deliberate check (insert case): a project YAML ending in a
        // trailing comment must keep that comment after the managed-bots
        // write seeds + splices the interfaces block.
        let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n\
                   # keep this trailing comment\n";
        let doc: team_core::yaml_edit::Document = src.parse().unwrap();
        let got = splice_project_manager_bot(doc, "TEAMCTL_TG_MANAGER_TOKEN")
            .unwrap()
            .to_string();
        assert!(
            got.contains("# keep this trailing comment"),
            "trailing comment eaten on insert (#319):\n{got}"
        );
    }

    #[ignore = "blocked on #319: yaml_edit::block_end_after eats the file-final \
                trailing comment on a leaf replace. The managed-bots wizard's own \
                writes don't create this shape (seeding appends `interfaces:` last, \
                pushing comments before it), so typical usage is safe — but a \
                --force re-run after an operator hand-adds a trailing comment hits \
                it. Un-ignore when #319 lands; this is its ready regression guard."]
    #[test]
    fn splice_manager_bot_preserves_file_final_comment_on_replace() {
        // #319 deliberate check (the actual trigger): manager_bot.token_env
        // is the file-final content block, followed only by a comment. A
        // force re-run REPLACES that leaf — the trailing comment must
        // survive (the comment-preserving substrate's whole contract).
        let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   interfaces:\n  telegram:\n    manager_bot:\n      token_env: OLD_TOKEN_ENV\n\
                   # final trailing comment\n";
        let doc: team_core::yaml_edit::Document = src.parse().unwrap();
        let got = splice_project_manager_bot(doc, "NEW_TOKEN_ENV")
            .unwrap()
            .to_string();
        assert_eq!(manager_bot_token_env(&got), "NEW_TOKEN_ENV");
        assert!(
            got.contains("# final trailing comment"),
            "file-final trailing comment eaten on replace (#319 fired for managed-bots path):\n{got}"
        );
    }

    #[test]
    fn managed_bot_write_validates_clean_and_round_trips() {
        // Integration: the managed-bots project write produces a YAML that
        // (1) validates clean through the real `team_core::validate` and
        // (2) round-trips so `Project::telegram().manager_bot` reads back.
        // The interactive wizard (stdin), the Telegram `getMe`/`/start`
        // HTTP, and the managed-bot creation flow are out of unit scope —
        // the managed-bot client is covered by #342's wiremock tests; this
        // pins the persisted schema the rest of the flow composes against.
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join(".team");
        let ess = crate::cmd::init::TEMPLATES
            .iter()
            .find(|t| t.key == "essentials")
            .expect("essentials template present");
        for (rel, content) in ess.files {
            let body = content
                .replace("{{project_id}}", "main")
                .replace("{{project_name}}", "Main");
            let path = team.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        let compose = Compose::load(&team).expect("essentials compose loads");
        let project_id = compose.projects[0].project.id.clone();

        upsert_project_manager_bot(&compose, &project_id, "TEAMCTL_TG_MANAGER_TOKEN")
            .expect("managed_bot write succeeds");

        let compose = Compose::load(&team).expect("compose reloads after managed_bot write");
        let errs = team_core::validate::validate(&compose);
        assert!(
            errs.is_empty(),
            "managed-bots YAML must validate clean: {errs:?}"
        );

        let mb = compose.projects[0]
            .telegram()
            .and_then(|t| t.manager_bot.as_ref())
            .expect("manager_bot round-trips through the schema");
        assert_eq!(mb.token_env, "TEAMCTL_TG_MANAGER_TOKEN");
    }
}
