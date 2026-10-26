use super::*;

/// Upsert a single env assignment and mirror it into the live process.
pub(super) fn upsert_env_var(root: &Path, key: &str, value: &str) -> Result<()> {
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
pub(super) fn upsert_project_manager_bot(
    compose: &Compose,
    project_id: &str,
    token_env: &str,
) -> Result<()> {
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
pub(super) fn splice_project_manager_bot(
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

pub(super) fn source_env_files(root: &Path) {
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

pub(super) fn write_env_file(root: &Path, k1: &str, v1: &str, k2: &str, v2: &str) -> Result<()> {
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

pub(super) fn upsert_manager_telegram(
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
pub(super) fn edit_manager_yaml(
    path: &Path,
    role: &str,
    token_env: &str,
    chats_env: &str,
) -> Result<()> {
    let doc = team_core::yaml_edit::load(path)?;

    // Sanity-check that the parent path exists before we splice. Errors
    // here match the pre-substrate behaviour callers rely on.
    let root = doc
        .as_mapping()
        .ok_or_else(|| anyhow!("root of {} is not a mapping", path.display()))?;
    let block = if root.get_mapping("agents").is_some() {
        "agents"
    } else {
        "managers"
    };
    let managers = root
        .get_mapping(block)
        .ok_or_else(|| anyhow!("`managers:` block missing in {}", path.display()))?;
    if managers.get_mapping(role).is_none() {
        return Err(anyhow!("manager `{role}` missing in {}", path.display()));
    }

    let doc = team_core::yaml_edit::set_nested_mapping(
        doc,
        &[block, role, "interfaces", "telegram"],
        &[("bot_token_env", token_env), ("chat_ids_env", chats_env)],
    )?;
    team_core::yaml_edit::save(&doc, path)?;
    Ok(())
}
