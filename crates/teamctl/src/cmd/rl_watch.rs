//! `teamctl rl-watch <project>:<agent> -- <bin> <args…>`
//!
//! Runs a runtime binary, streams its stdout/stderr through to our own
//! stdout (so the agent's tmux pane shows what's happening), and tests each
//! line against the runtime's `rate_limit_patterns`. On a hit:
//!
//! 1. Insert a row into the `rate_limits` table.
//! 2. Run the agent's `on_rate_limit` hook chain (or the global default).
//! 3. Sleep until the captured `resets_at` (with a small jitter) or
//!    `fallback_wait_seconds`.
//! 4. Exit 0 — the surrounding `agent-wrapper.sh` loop respawns the runtime
//!    *after* the limit window has cleared.
//!
//! If the runtime exits cleanly without a rate-limit signature, we exit
//! with the runtime's own status code so the wrapper handles that path.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Local, NaiveTime, TimeZone, Timelike, Utc};
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
            "runtime `{}` for agent `{target}` has no descriptor in runtimes/",
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

    // Spawn the runtime with merged stdout+stderr piped in.
    let mut child = Command::new(bin)
        .args(bin_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn runtime `{bin}`"))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Two reader threads — one per stream — both publishing matched events
    // back to the main thread via an mpsc channel.
    let (tx, rx) = std::sync::mpsc::channel::<RlEvent>();

    let stdout_tx = tx.clone();
    let stdout_pats = patterns.clone();
    thread::spawn(move || stream_loop(stdout, stdout_pats, stdout_tx, false));

    let stderr_tx = tx.clone();
    let stderr_pats = patterns.clone();
    thread::spawn(move || stream_loop(stderr, stderr_pats, stderr_tx, true));

    drop(tx); // last sender dies when both reader threads finish

    let mut hit: Option<RlEvent> = None;
    while let Ok(ev) = rx.recv() {
        if hit.is_none() {
            hit = Some(ev);
            // Don't kill child here — let it exit on its own. Most runtimes
            // do once they print the rate-limit message.
        }
    }

    let status = child.wait().context("wait runtime")?;

    if let Some(ev) = hit {
        on_hit(&compose, &db_path, target, &handle.spec.runtime, &ev)?;
        return Ok(()); // wrapper re-spawns
    }

    // No rate-limit detected — exit with the runtime's own status code.
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("runtime exited {}", code_str(&status)))
    }
}

fn code_str(status: &ExitStatus) -> String {
    match status.code() {
        Some(c) => c.to_string(),
        None => "<signal>".into(),
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

fn stream_loop<R: std::io::Read + Send + 'static>(
    src: R,
    patterns: Vec<CompiledPattern>,
    tx: std::sync::mpsc::Sender<RlEvent>,
    is_stderr: bool,
) {
    let reader = BufReader::new(src);
    for line in reader.lines().map_while(Result::ok) {
        // Passthrough so the agent's tmux pane keeps showing the live output.
        if is_stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
        for p in &patterns {
            if p.matcher.is_match(&line) {
                let resets_at = parse_resets(&line, p);
                let _ = tx.send(RlEvent {
                    raw: line.clone(),
                    resets_at,
                });
                break;
            }
        }
    }
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
    conn.execute_batch(team_core::mailbox::SCHEMA)?;
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
            let mut cmd = Command::new("curl");
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
            let st = Command::new(bin)
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
}
