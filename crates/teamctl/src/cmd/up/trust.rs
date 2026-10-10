use super::*;

/// Pre-accept Claude Code's per-workspace trust dialog for every cwd that
/// will host a `claude-code` agent. Without this, the runtime blocks on a
/// "Do you trust this folder?" prompt the moment it boots, defeating the
/// "agents start working when teamctl up runs" model.
///
/// Running `teamctl up` is itself an explicit "I trust this directory"
/// signal -- the user is about to launch AI agents with tool access in
/// it -- so we record that consent in `~/.claude.json` once instead of
/// making them click through the dialog every restart.
pub(super) fn ensure_claude_trust(compose: &Compose) -> Result<()> {
    ensure_claude_trust_inner(compose, None)
}

pub(super) fn ensure_claude_trust_for_project(compose: &Compose, project_id: &str) -> Result<()> {
    ensure_claude_trust_inner(compose, Some(project_id))
}

pub(super) fn ensure_claude_trust_inner(compose: &Compose, project_id: Option<&str>) -> Result<()> {
    let cwds: BTreeSet<PathBuf> = compose
        .agents()
        .filter(|h| project_id.is_none_or(|id| h.project == id))
        .filter(|h| h.spec.runtime == "claude-code")
        .filter_map(|h| {
            let project = compose
                .projects
                .iter()
                .find(|p| p.project.id == h.project)?;
            let cwd = if project.project.cwd.is_absolute() {
                project.project.cwd.clone()
            } else {
                compose.root.join(&project.project.cwd)
            };
            cwd.canonicalize().ok().or(Some(cwd))
        })
        .collect();

    if cwds.is_empty() {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    let config_path = home.join(".claude.json");
    let _lock = team_core::persistence::ProcessLock::acquire(
        &home.join(".claude.json.teamctl.lock.sqlite"),
    )?;

    let mut config: serde_json::Value = match fs::read_to_string(&config_path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| {
            format!(
                "parse {} (preserving invalid config)",
                config_path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e).with_context(|| format!("read {}", config_path.display())),
    };
    anyhow::ensure!(
        config.is_object(),
        "{} must contain a JSON object",
        config_path.display()
    );
    if !config
        .get("projects")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        config["projects"] = serde_json::json!({});
    }
    let projects = config["projects"].as_object_mut().unwrap();

    let mut newly_trusted = Vec::new();
    for cwd in &cwds {
        let key = cwd.display().to_string();
        let entry = projects
            .entry(key.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let obj = entry.as_object_mut().unwrap();
        let already = matches!(
            obj.get("hasTrustDialogAccepted"),
            Some(serde_json::Value::Bool(true))
        );
        if !already {
            obj.insert(
                "hasTrustDialogAccepted".into(),
                serde_json::Value::Bool(true),
            );
            newly_trusted.push(key);
        }
    }

    if newly_trusted.is_empty() {
        return Ok(());
    }

    // Write atomically so a concurrent claude reader never sees a
    // half-written config.
    team_core::persistence::atomic_write(
        &config_path,
        serde_json::to_string_pretty(&config)?.as_bytes(),
    )?;

    // Transparency: editing the user's Claude Code config on their behalf is
    // not something we do silently. Tell them — at `up`, where a human is at
    // the terminal — exactly which folders we marked trusted and where, and
    // why. (This is a notice, not a prompt: agent panes stay non-interactive.)
    // User-facing copy: no em-dash (owner house style); singular/plural agree.
    // Wording is neda's polish pass (msg 1634).
    let n = newly_trusted.len();
    let folder_word = if n == 1 { "folder" } else { "folders" };
    let delete_phrase = if n == 1 { "that key" } else { "those keys" };
    eprintln!();
    eprintln!("trust · marked {n} {folder_word} trusted in your Claude Code config");
    eprintln!("        so agents don't stall on Claude's \"trust this folder\" prompt:");
    for path in &newly_trusted {
        eprintln!("          • {path}");
    }
    eprintln!(
        "        config: {} (key: hasTrustDialogAccepted)",
        config_path.display()
    );
    eprintln!("        running `teamctl up` granted this trust; delete {delete_phrase} to undo.");
    eprintln!();
    Ok(())
}
