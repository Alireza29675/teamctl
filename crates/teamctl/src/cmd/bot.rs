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
        None => all_managers.clone(),
    };

    println!("teamctl bot setup");
    println!("─────────────────");

    let mut configured = 0usize;
    let mut skipped = 0usize;
    for mgr in &filtered {
        match wizard_one(root, &compose, mgr, force)? {
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
        let t = prompt("Paste bot token: ")?.trim().to_string();
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

    // Locate the project file path via global.projects[].file.
    let proj_ref = compose
        .global
        .projects
        .iter()
        .find(|r| {
            // Match the parsed project at the same index by reading the
            // file's `project.id` would require re-parsing; cheaper: try
            // each candidate file and pick the one whose project.id
            // matches.
            let p = compose.root.join(&r.file);
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|raw| serde_yaml::from_str::<serde_yaml::Value>(&raw).ok())
                .and_then(|v| {
                    v.get("project")
                        .and_then(|p| p.get("id"))
                        .and_then(|id| id.as_str())
                        .map(|s| s == project_id)
                })
                .unwrap_or(false)
        })
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
    pub session: String,
    pub mailbox: PathBuf,
    pub token_env: String,
    pub chats_env: String,
}

pub fn bot_specs(compose: &Compose) -> Vec<BotSpec> {
    let prefix = &compose.global.supervisor.tmux_prefix;
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mut out = Vec::new();
    for proj in &compose.projects {
        for (role, agent) in &proj.managers {
            if let Some(tg) = agent.telegram() {
                let mgr = format!("{}:{}", proj.project.id, role);
                out.push(BotSpec {
                    session: bot_session_name(prefix, &mgr),
                    mailbox: mailbox.clone(),
                    token_env: tg.bot_token_env.clone(),
                    chats_env: tg.chat_ids_env.clone(),
                    manager: mgr,
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

    let cmd = format!(
        "{bin} --mailbox {mb} --token {tok} --authorized-chat-ids {chats} --manager {mgr}",
        bin = shlex_quote(&team_bot_bin.display().to_string()),
        mb = shlex_quote(&spec.mailbox.display().to_string()),
        tok = shlex_quote(&token),
        chats = shlex_quote(&chats),
        mgr = shlex_quote(&spec.manager),
    );
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
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
}
