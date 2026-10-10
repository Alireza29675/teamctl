use super::*;

pub(super) fn record_hit(
    db_path: &Path,
    agent_id: &str,
    runtime: &str,
    ev: &RlEvent,
) -> Result<(i64, f64)> {
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
    Ok((row_id, hit_at))
}

pub(super) fn update_hit(db_path: &Path, row_id: i64, event: &RlEvent) -> Result<()> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute(
        "UPDATE rate_limits SET resets_at=?1, raw_match=?2 WHERE id=?3",
        params![event.resets_at, event.raw, row_id],
    )?;
    Ok(())
}

pub(super) fn on_hit(
    compose: &Compose,
    db_path: &Path,
    agent_id: &str,
    runtime: &str,
    ev: &RlEvent,
    row_id: i64,
    hit_at: f64,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_secs(5))?;

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
pub(super) struct HookContext {
    agent_id: String,
    runtime: String,
    hit_at: f64,
    pub(super) resets_at: Option<f64>,
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

pub(super) fn run_hook(hook: &RateLimitHook, bag: &HookContext, db_path: &Path) -> Result<()> {
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

pub(super) fn wait_for_reset(
    cfg: &team_core::compose::RateLimits,
    resets_at: Option<f64>,
    hit_at: f64,
) {
    let secs = match resets_at {
        Some(ts) => (ts - hit_at).max(0.0) as u64 + 5, // 5s jitter past reset
        None => cfg.fallback_wait_seconds,
    };
    eprintln!("[rl-watch] sleeping {secs}s before letting wrapper respawn the runtime");
    thread::sleep(Duration::from_secs(secs));
}
