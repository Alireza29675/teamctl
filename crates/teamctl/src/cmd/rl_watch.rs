//! `teamctl rl-watch <project>:<agent> -- <bin> <args…>`
//!
//! Spawns a runtime binary under a *pseudo-terminal* so it sees a real TTY
//! (interactive Claude Code REPL, Codex, etc. all need this), forwards
//! the wrapper's own stdin into the pty so attached operators can drive
//! the session, copies pty output back to the wrapper's stdout, AND
//! scans each line for the runtime's `rate_limit_patterns`. On a hit:
//!
//! 1. Insert a row into the `rate_limits` table.
//! 2. Run the agent's `on_rate_limit` hook chain (or the global default).
//! 3. Sleep until the captured `resets_at` (with a small jitter) or
//!    `fallback_wait_seconds`.
//! 4. Exit 0 — the surrounding `agent-wrapper.sh` loop respawns the runtime
//!    *after* the limit window has cleared.
//!
//! Without the pty wrap, runtimes detect non-TTY stdio and silently drop
//! into one-shot/print mode, exit immediately, and the wrapper enters a
//! 5-second restart loop -- which was the v0.1 behaviour.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Local, NaiveTime, TimeZone, Timelike, Utc};
use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use regex::Regex;
use rusqlite::{params, Connection};
use team_core::compose::{Compose, RateLimitHook};
use team_core::runtimes::RateLimitPattern;

pub fn run(root: &Path, target: &str, runtime_args: &[String]) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let runtimes = team_core::runtimes::load_all(&compose.root)?;
    let Some(rt_def) = runtimes.get(&handle.spec.runtime) else {
        bail!(
            "runtime `{}` for agent `{target}` is unknown -- not built in and no `<root>/runtimes/{}.yaml` override found",
            handle.spec.runtime,
            handle.spec.runtime
        );
    };

    if runtime_args.is_empty() {
        bail!("rl-watch needs a runtime command after `--`");
    }
    let bin = &runtime_args[0];
    let bin_args = &runtime_args[1..];

    let patterns = compile_patterns(&rt_def.rate_limit_patterns)?;
    let db_path = compose.root.join(&compose.global.broker.path);

    tracing::info!(
        agent = %target,
        runtime = %handle.spec.runtime,
        "rl-watch starting; {} pattern(s)",
        patterns.len()
    );

    // Open a pty pair sized to our controlling terminal (or 80x24 fallback).
    let pty_size = current_winsize();
    let pair = native_pty_system()
        .openpty(pty_size)
        .context("openpty for runtime")?;

    // Build the child command. CommandBuilder doesn't inherit env by default;
    // copy ours through so the runtime sees PATH, HOME, ANTHROPIC_*, etc.
    let mut cmd = CommandBuilder::new(bin);
    for arg in bin_args {
        cmd.arg(arg);
    }
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn runtime `{bin}` under pty"))?;
    drop(pair.slave); // close our copy of the slave fd

    let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
    let mut writer = pair.master.take_writer().context("take pty writer")?;

    // If our stdin is a TTY, switch it to raw mode so individual keystrokes
    // reach the child immediately (instead of being buffered until newline
    // by line discipline). Restored on drop of the guard.
    let _termios_guard = if std::io::stdin().is_terminal() {
        TermiosGuard::new()
    } else {
        TermiosGuard::noop()
    };

    let child_alive = Arc::new(AtomicBool::new(true));

    // Stdin -> pty writer thread. Exits when stdin hits EOF or child dies.
    let stdin_alive = child_alive.clone();
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        while stdin_alive.load(Ordering::SeqCst) {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if writer.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                Err(_) => break,
            }
        }
    });

    // Main loop: pty reader -> stdout, with line-buffered pattern scan.
    let mut buf = [0u8; 4096];
    let mut line_buf: Vec<u8> = Vec::new();
    let stdout = std::io::stdout();
    let mut hit: Option<RlEvent> = None;

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                {
                    let mut out = stdout.lock();
                    let _ = out.write_all(&buf[..n]);
                    let _ = out.flush();
                }
                if hit.is_none() {
                    for &b in &buf[..n] {
                        match b {
                            b'\n' | b'\r' => {
                                if let Some(ev) = scan_line(&line_buf, &patterns) {
                                    hit = Some(ev);
                                    break;
                                }
                                line_buf.clear();
                            }
                            _ => line_buf.push(b),
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }

    child_alive.store(false, Ordering::SeqCst);
    let status = child.wait().context("wait runtime")?;

    if let Some(ev) = hit {
        on_hit(&compose, &db_path, target, &handle.spec.runtime, &ev)?;
        return Ok(()); // wrapper re-spawns
    }

    // No rate-limit detected — exit with the runtime's own status code.
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("runtime exited {}", status_str(&status)))
    }
}

fn scan_line(buf: &[u8], patterns: &[CompiledPattern]) -> Option<RlEvent> {
    if buf.is_empty() {
        return None;
    }
    // Strip basic ANSI CSI/OSC sequences before matching so escape codes
    // baked into the runtime's status line don't defeat the regex.
    let stripped = strip_ansi(buf);
    let line = String::from_utf8_lossy(&stripped).into_owned();
    for p in patterns {
        if p.matcher.is_match(&line) {
            let resets_at = parse_resets(&line, p);
            return Some(RlEvent {
                raw: line,
                resets_at,
            });
        }
    }
    None
}

fn strip_ansi(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == 0x1b && i + 1 < input.len() {
            match input[i + 1] {
                b'[' => {
                    // CSI: skip until a byte in 0x40..=0x7E
                    i += 2;
                    while i < input.len() && !(0x40..=0x7e).contains(&input[i]) {
                        i += 1;
                    }
                    if i < input.len() {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: skip until BEL (0x07) or ST (ESC \)
                    i += 2;
                    while i < input.len() {
                        if input[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Other ESC X — skip the two bytes
                    i += 2;
                }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

fn status_str(status: &ExitStatus) -> String {
    if status.success() {
        "0".into()
    } else {
        format!("{}", status.exit_code())
    }
}

fn current_winsize() -> PtySize {
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        let fd = libc::STDIN_FILENO;
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            return PtySize {
                rows: ws.ws_row,
                cols: ws.ws_col,
                pixel_width: ws.ws_xpixel,
                pixel_height: ws.ws_ypixel,
            };
        }
    }
    PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// RAII guard that puts stdin into raw mode on construction and restores
/// the saved termios on drop. `noop()` constructs a guard that does
/// nothing (used when stdin is not a TTY).
struct TermiosGuard {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl TermiosGuard {
    #[cfg(unix)]
    fn new() -> Self {
        unsafe {
            let fd = libc::STDIN_FILENO;
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return TermiosGuard { saved: None };
            }
            let saved = termios;
            libc::cfmakeraw(&mut termios);
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return TermiosGuard { saved: None };
            }
            TermiosGuard { saved: Some(saved) }
        }
    }

    #[cfg(not(unix))]
    fn new() -> Self {
        TermiosGuard {}
    }

    fn noop() -> Self {
        #[cfg(unix)]
        {
            TermiosGuard { saved: None }
        }
        #[cfg(not(unix))]
        {
            TermiosGuard {}
        }
    }
}

#[cfg(unix)]
impl Drop for TermiosGuard {
    fn drop(&mut self) {
        if let Some(t) = self.saved {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
}

#[derive(Clone)]
struct CompiledPattern {
    matcher: Regex,
    resets_at: Option<Regex>,
    resets_in: Option<Regex>,
}

fn compile_patterns(src: &[RateLimitPattern]) -> Result<Vec<CompiledPattern>> {
    src.iter()
        .map(|p| {
            Ok(CompiledPattern {
                matcher: Regex::new(&p.r#match)
                    .with_context(|| format!("compile match regex `{}`", p.r#match))?,
                resets_at: p
                    .resets_at_capture
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .context("compile resets_at_capture")?,
                resets_in: p
                    .resets_in_capture
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .context("compile resets_in_capture")?,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct RlEvent {
    raw: String,
    resets_at: Option<f64>,
}

fn parse_resets(line: &str, p: &CompiledPattern) -> Option<f64> {
    if let Some(re) = &p.resets_at {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1) {
                return parse_clock_time(m.as_str());
            }
        }
    }
    if let Some(re) = &p.resets_in {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1) {
                if let Some(secs) = parse_duration(m.as_str()) {
                    return Some(now() + secs as f64);
                }
            }
        }
    }
    None
}

/// Parse "4pm", "16:00", "16:00 UTC", "4:30 pm" → next future occurrence (UNIX seconds).
fn parse_clock_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let formats = ["%I%P", "%I%p", "%I:%M%P", "%I:%M%p", "%H:%M"];
    let candidate = s
        .split_whitespace()
        .next()
        .unwrap_or(s)
        .to_lowercase()
        .replace(' ', "");
    let now = Local::now();
    for f in formats {
        if let Ok(t) = NaiveTime::parse_from_str(&candidate, f) {
            let mut d = now
                .date_naive()
                .and_hms_opt(t.hour(), t.minute(), 0)
                .unwrap();
            // If the time has already passed today, assume tomorrow.
            if d <= now.naive_local() {
                d += chrono::Duration::days(1);
            }
            let local = Local.from_local_datetime(&d).single()?;
            return Some(local.with_timezone(&Utc).timestamp() as f64);
        }
    }
    None
}

/// Parse "5h", "5h 15m", "30m", "120s", "2 hours" → seconds.
fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let mut total: u64 = 0;
    let mut buf = String::new();
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c.is_ascii_digit() {
            buf.push(c);
            continue;
        }
        if buf.is_empty() {
            continue;
        }
        let n: u64 = buf.parse().ok()?;
        buf.clear();
        // Read the unit greedily.
        let mut unit = String::from(c);
        while let Some(&p) = iter.peek() {
            if p.is_ascii_alphabetic() {
                unit.push(p);
                iter.next();
            } else {
                break;
            }
        }
        let unit = unit.trim();
        let mul = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            _ => return None,
        };
        total += n * mul;
    }
    if !buf.is_empty() {
        // bare number: treat as seconds
        total += buf.parse::<u64>().ok()?;
    }
    if total == 0 {
        None
    } else {
        Some(total)
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn on_hit(
    compose: &Compose,
    db_path: &Path,
    agent_id: &str,
    runtime: &str,
    ev: &RlEvent,
) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;
    let hit_at = now();
    conn.execute(
        "INSERT INTO rate_limits (agent_id, runtime, hit_at, resets_at, raw_match)
         VALUES (?1,?2,?3,?4,?5)",
        params![agent_id, runtime, hit_at, ev.resets_at, ev.raw],
    )?;
    let row_id = conn.last_insert_rowid();

    eprintln!("[rl-watch] rate-limit hit on {agent_id}: {}", ev.raw);
    if let Some(ts) = ev.resets_at {
        let resets_local = Local.timestamp_opt(ts as i64, 0).single();
        eprintln!(
            "[rl-watch] resets at {} (in {} s)",
            resets_local
                .map(|d| d.format("%Y-%m-%d %H:%M:%S %Z").to_string())
                .unwrap_or_else(|| "<unparsed>".into()),
            (ts - hit_at).max(0.0) as u64
        );
    }

    // Resolve the hook chain. Per-agent override beats the global default;
    // if both are empty, fall back to ["wait"].
    let agent_chain = compose
        .agents()
        .find(|h| h.id() == agent_id)
        .and_then(|h| h.spec.on_rate_limit.clone());
    let chain = agent_chain
        .or_else(|| {
            let d = compose.global.rate_limits.default_on_hit.clone();
            (!d.is_empty()).then_some(d)
        })
        .unwrap_or_else(|| vec!["wait".into()]);

    let bag = HookContext {
        agent_id: agent_id.into(),
        runtime: runtime.into(),
        hit_at,
        resets_at: ev.resets_at,
        raw_match: ev.raw.clone(),
    };

    for name in chain {
        match name.as_str() {
            "wait" => wait_for_reset(&compose.global.rate_limits, ev.resets_at, hit_at),
            other => {
                if let Some(hook) = compose
                    .global
                    .rate_limits
                    .hooks
                    .iter()
                    .find(|h| h.name == other)
                {
                    if let Err(e) = run_hook(hook, &bag, db_path) {
                        eprintln!("[rl-watch] hook {} failed: {e}", hook.name);
                    }
                } else {
                    eprintln!("[rl-watch] no rate_limits.hook named `{other}` — skipping");
                }
            }
        }
    }

    conn.execute(
        "UPDATE rate_limits SET handled_at = ?1 WHERE id = ?2",
        params![now(), row_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
struct HookContext {
    agent_id: String,
    runtime: String,
    hit_at: f64,
    resets_at: Option<f64>,
    raw_match: String,
}

impl HookContext {
    fn substitute(&self, s: &str) -> String {
        let resets_at = self
            .resets_at
            .map(|t| t.to_string())
            .unwrap_or_else(|| "unknown".into());
        let resets_at_local = self
            .resets_at
            .and_then(|t| Local.timestamp_opt(t as i64, 0).single())
            .map(|d| d.format("%H:%M %Z").to_string())
            .unwrap_or_else(|| "unknown".into());
        s.replace("{agent}", &self.agent_id)
            .replace("{runtime}", &self.runtime)
            .replace("{hit_at}", &self.hit_at.to_string())
            .replace("{resets_at}", &resets_at)
            .replace("{resets_at_local}", &resets_at_local)
            .replace("{raw_match}", &self.raw_match)
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent": self.agent_id,
            "runtime": self.runtime,
            "hit_at": self.hit_at,
            "resets_at": self.resets_at,
            "raw_match": self.raw_match,
        })
    }
}

fn run_hook(hook: &RateLimitHook, bag: &HookContext, db_path: &Path) -> Result<()> {
    match hook.action.as_str() {
        "send" => {
            let to = hook
                .to
                .as_ref()
                .ok_or_else(|| anyhow!("hook {} missing `to`", hook.name))?;
            let template = hook
                .template
                .as_deref()
                .unwrap_or("rate-limit hit on {agent}; resets {resets_at_local}");
            let text = bag.substitute(template);
            let project = to.split_once(':').map(|(p, _)| p).unwrap_or("");
            let conn = Connection::open(db_path)?;
            conn.execute(
                "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
                 VALUES (?1, 'rl-watch', ?2, ?3, ?4)",
                params![project, to, text, now()],
            )?;
            eprintln!("[rl-watch] hook {}: send → {to}", hook.name);
        }
        "webhook" => {
            let url = match (&hook.url, &hook.url_env) {
                (Some(u), _) => u.clone(),
                (None, Some(env)) => std::env::var(env)
                    .with_context(|| format!("read env var {env} for hook {}", hook.name))?,
                (None, None) => bail!("hook {} needs `url` or `url_env`", hook.name),
            };
            let method = hook.method.as_deref().unwrap_or("POST");
            let body = bag.to_json().to_string();
            let mut cmd = std::process::Command::new("curl");
            cmd.args([
                "-fsS",
                "-X",
                method,
                "-H",
                "content-type: application/json",
                "--data",
                &body,
                &url,
            ]);
            let st = cmd
                .status()
                .with_context(|| format!("invoke curl for hook {}", hook.name))?;
            anyhow::ensure!(st.success(), "curl exited {st}");
            eprintln!("[rl-watch] hook {}: webhook {method} {url}", hook.name);
        }
        "run" => {
            let mut iter = hook.command.iter();
            let bin = iter
                .next()
                .ok_or_else(|| anyhow!("hook {} `command` is empty", hook.name))?;
            let args: Vec<String> = iter.map(|a| bag.substitute(a)).collect();
            let st = std::process::Command::new(bin)
                .args(&args)
                .status()
                .with_context(|| format!("run command for hook {}", hook.name))?;
            anyhow::ensure!(st.success(), "command exited {st}");
            eprintln!("[rl-watch] hook {}: ran {} {:?}", hook.name, bin, args);
        }
        "wait" => {
            // Treated specially in the chain dispatcher above; reached here
            // only if a user named a hook "wait" with action=wait. Honour it.
            wait_for_reset(
                &team_core::compose::RateLimits::default(),
                bag.resets_at,
                bag.hit_at,
            );
        }
        other => bail!("unknown hook action `{other}`"),
    }
    Ok(())
}

fn wait_for_reset(cfg: &team_core::compose::RateLimits, resets_at: Option<f64>, hit_at: f64) {
    let secs = match resets_at {
        Some(ts) => (ts - hit_at).max(0.0) as u64 + 5, // 5s jitter past reset
        None => cfg.fallback_wait_seconds,
    };
    eprintln!("[rl-watch] sleeping {secs}s before letting wrapper respawn the runtime");
    thread::sleep(Duration::from_secs(secs));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parses_compound() {
        assert_eq!(parse_duration("5h 15m"), Some(5 * 3600 + 15 * 60));
        assert_eq!(parse_duration("30m"), Some(30 * 60));
        assert_eq!(parse_duration("120s"), Some(120));
        assert_eq!(parse_duration("2 hours"), Some(2 * 3600));
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn compile_patterns_works() {
        let v = vec![RateLimitPattern {
            r#match: "(?i)limit reached".into(),
            resets_at_capture: Some("(?i)at ([0-9]+(?:am|pm))".into()),
            resets_in_capture: None,
        }];
        let c = compile_patterns(&v).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].matcher.is_match("Limit reached, please wait"));
    }

    #[test]
    fn strip_ansi_csi() {
        let input = b"\x1b[31mhello\x1b[0m world";
        assert_eq!(strip_ansi(input), b"hello world");
    }

    #[test]
    fn strip_ansi_osc() {
        let input = b"\x1b]0;title\x07after";
        assert_eq!(strip_ansi(input), b"after");
    }

    #[test]
    fn scan_line_matches_with_ansi_codes() {
        let patterns = compile_patterns(&[RateLimitPattern {
            r#match: "(?i)limit reached".into(),
            resets_at_capture: None,
            resets_in_capture: None,
        }])
        .unwrap();
        let line = b"\x1b[33mLimit reached!\x1b[0m";
        assert!(scan_line(line, &patterns).is_some());
    }
}
