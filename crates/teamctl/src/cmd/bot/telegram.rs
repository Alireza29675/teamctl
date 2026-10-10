use super::*;

// ── Telegram HTTP via curl ──────────────────────────────────────────

#[derive(Debug)]
pub(super) struct TelegramUser {
    pub(super) username: Option<String>,
    pub(super) first_name: Option<String>,
}

pub(super) fn telegram_get_me(token: &str) -> Result<TelegramUser> {
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

pub(super) fn poll_for_start(token: &str, deadline: Duration) -> Result<i64> {
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

pub(super) fn curl_get(url: &str) -> Result<String> {
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
