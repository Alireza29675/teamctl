use super::*;

/// #383 Phase 3a: build Claude Code's `--agents` inline JSON for one agent
/// compose-root-relative markdown file with standard sub-agent frontmatter
/// (`name`, `description`, optional `tools`, `model`) and a body that
/// becomes the sub-agent's system `prompt`. The result is the
/// `{ "<name>": { description, prompt, [tools], [model] } }` object the
/// `--agents` flag consumes — the only cwd-stationary way to scope
/// sub-agents per agent (no arbitrary-path flag exists; see the Phase-1
/// spike). Returns `Ok(None)` when none are declared (→ no `--agents`
/// flag) or the runtime isn't claude-code (logs an "unsupported" warning,
/// claude-only v1); `Err` if a source is unreadable or its frontmatter is
/// invalid, so a typo fails the apply loudly rather than dropping a
/// sub-agent silently.
pub fn render_subagents(compose: &Compose, h: AgentHandle<'_>) -> io::Result<Option<String>> {
    if h.spec.subagents.is_empty() {
        return Ok(None);
    }
    if h.spec.runtime != "claude-code" {
        tracing::warn!(
            target: "team-core::render",
            "agent `{}:{}` declares {} sub-agent(s) but runtime `{}` does not support sub-agents (claude-code only); ignoring",
            h.project,
            h.agent,
            h.spec.subagents.len(),
            h.spec.runtime
        );
        return Ok(None);
    }

    let mut map = serde_json::Map::new();
    for rel in &h.spec.subagents {
        let abs = compose.root.join(rel);
        let raw = std::fs::read_to_string(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read sub-agent source {}: {e}", abs.display()),
            )
        })?;
        let (fm, body) = parse_subagent(&raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse sub-agent {}: {e}", abs.display()),
            )
        })?;
        // Name from frontmatter, else the file stem (so `agents/foo.md`
        // without an explicit `name:` registers as sub-agent `foo`).
        let name = fm.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            rel.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let mut entry = serde_json::json!({
            "description": fm.description,
            "prompt": body,
        });
        if let Some(tools) = fm.tools {
            let list = tools.into_list();
            if !list.is_empty() {
                entry["tools"] = serde_json::json!(list);
            }
        }
        if let Some(model) = fm.model.filter(|m| !m.trim().is_empty()) {
            entry["model"] = serde_json::Value::String(model);
        }
        map.insert(name, entry);
    }
    Ok(Some(
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).expect("json"),
    ))
}

/// Write (or clear) the per-agent `--agents` JSON file. Mirrors
/// [`write_role_prompt_concat`]: the scoped + full render paths both call
/// it so a `subagents:` edit flows into the agent at the next render. When
/// the agent declares no sub-agents (or isn't claude-code) the file is
/// removed if present, so a stale `--agents` set never lingers across a
/// reload that dropped them.
pub fn write_subagents_json(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let dest = subagents_json_path(&compose.root, h.project, h.agent);
    match render_subagents(compose, h)? {
        Some(json) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::persistence::atomic_write(&dest, json.as_bytes())
        }
        None => match std::fs::remove_file(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

/// Materialize (or clear) the per-agent skills scope for one agent (#383
/// Phase 3b). For a claude-code agent declaring `skills:`, this creates
/// `state/agent-scope/<project>-<agent>/.claude/skills/` and symlinks each
/// declared skill directory into it (link name = the skill dir's basename),
/// so `claude --add-dir <scope>` surfaces them additively atop the project
/// `.claude/skills/`. Mirrors [`write_subagents_json`]: the scoped + full
/// render paths both call it, and the skills dir is rebuilt from scratch
/// every render so a renamed or dropped skill never lingers. When the agent
/// declares no skills (or isn't claude-code) the scope dir is removed if
/// present.
///
/// Symlink targets are absolute (compose-root-relative input resolved
/// against `compose.root`); a missing source becomes a dangling link rather
/// than an error, matching how `role_prompt`/`hooks` treat not-yet-created
/// paths (existence checks across all path-typed fields are a tracked
/// follow-up). Clearing always unlinks entries individually — render never
/// hands a symlink to `remove_dir_all`, so a skill's real files are never
/// followed or deleted.
pub fn write_agent_skills(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let scope = agent_scope_dir(&compose.root, h.project, h.agent);
    let skills_dir = scope.join(".claude/skills");

    if h.spec.runtime != "claude-code" || h.spec.skills.is_empty() {
        if h.spec.runtime != "claude-code" && !h.spec.skills.is_empty() {
            // Skills are a Claude-Code concept; surface a warning so a
            // declared-but-ignored skill isn't silently dropped (claude-
            // only v1, same shape as hooks/sub-agents).
            tracing::warn!(
                target: "team-core::render",
                "agent `{}:{}` declares {} skill(s) but runtime `{}` does not support skills (claude-code only); ignoring",
                h.project,
                h.agent,
                h.spec.skills.len(),
                h.spec.runtime
            );
        }
        // Clear a stale scope dir so dropped skills don't linger across a
        // reload that removed them.
        return remove_scope_dir(&scope);
    }

    // Rebuild from scratch each render: clear the existing links (each is a
    // symlink we created — unlink it, never recurse into its target) then
    // re-create the current set.
    clear_skills_dir(&skills_dir)?;
    std::fs::create_dir_all(&skills_dir)?;
    for rel in &h.spec.skills {
        // Link name is the skill directory's basename — Claude Code
        // discovers `.claude/skills/<name>/SKILL.md`.
        let Some(name) = rel.file_name() else {
            continue; // path ending in `..` / root has no skill name
        };
        let link = skills_dir.join(name);
        // Last-wins on a duplicate basename (consistent with sub-agents'
        // name-keyed map): drop any link already placed for this name.
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(compose.root.join(rel), &link)?;
    }
    Ok(())
}

/// Remove the per-agent scope dir if present. Clears the managed symlinks
/// individually first, so `remove_dir_all` only ever sees plain
/// directories — it never gets a symlink entry that could be followed into
/// a skill's real files. No-op when the dir doesn't exist.
fn remove_scope_dir(scope: &Path) -> io::Result<()> {
    clear_skills_dir(&scope.join(".claude/skills"))?;
    match std::fs::remove_dir_all(scope) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove every entry in the per-agent skills dir. Each entry is a symlink
/// render created, so we `remove_file` (unlink) it — never recursing into
/// the skill's real contents. No-op when the dir doesn't exist yet.
fn clear_skills_dir(skills_dir: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() || meta.is_file() {
            std::fs::remove_file(&path)?;
        } else {
            // Defensive: we only create symlinks here, but if a real
            // subdir somehow appears, clear it without following links.
            std::fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// Parsed frontmatter of a sub-agent markdown file. Mirrors the fields
/// Claude Code's own `.claude/agents/*.md` use; unknown keys are ignored.
#[derive(serde::Deserialize)]
pub(super) struct SubagentFrontmatter {
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) description: String,
    #[serde(default)]
    pub(super) tools: Option<Tools>,
    #[serde(default)]
    pub(super) model: Option<String>,
}

/// `tools:` accepts either Claude Code's comma-separated string form
/// (`Read, Grep`) or a YAML list (`[Read, Grep]`); both normalize to the
/// JSON array `--agents` expects.
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub(super) enum Tools {
    List(Vec<String>),
    Csv(String),
}

impl Tools {
    pub(super) fn into_list(self) -> Vec<String> {
        let raw = match self {
            Tools::List(v) => v,
            Tools::Csv(s) => s.split(',').map(str::to_string).collect(),
        };
        raw.into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Split a sub-agent markdown file into (frontmatter, body). Expects the
/// standard `---\n<yaml>\n---\n<body>` layout; the body is everything after
/// the closing delimiter, trimmed of surrounding blank lines.
pub(super) fn parse_subagent(raw: &str) -> Result<(SubagentFrontmatter, String), String> {
    let after_open = raw
        .strip_prefix("---")
        .ok_or("missing opening `---` frontmatter delimiter")?;
    let (yaml, body) = after_open
        .split_once("\n---")
        .ok_or("missing closing `---` frontmatter delimiter")?;
    let fm: SubagentFrontmatter =
        serde_yaml::from_str(yaml.trim()).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;
    let body = body.trim_start_matches(['\r', '\n']).trim_end().to_string();
    Ok((fm, body))
}
