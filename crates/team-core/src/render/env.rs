use super::*;

pub(super) fn render_env(compose: &Compose, h: AgentHandle<'_>) -> String {
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .expect("agent belongs to a loaded project");
    let mailbox = compose.root.join(&compose.global.broker.path);
    let mcp = mcp_path(&compose.root, h.project, h.agent);
    let prompt = system_prompt_path(compose, h)
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut s = String::new();
    let runtimes = crate::runtimes::load_all(&compose.root).unwrap_or_default();
    let runtime = runtimes.get(&h.spec.runtime);
    if let Some(runtime) = runtime {
        for (key, value) in &runtime.env {
            s.push_str(&format!("{key}={value}\n"));
        }
        s.push_str(&format!("RUNTIME_BINARY={}\n", runtime.binary));
        s.push_str(&format!(
            "RUNTIME_LAUNCHER={}\n",
            runtime_launcher_path(&compose.root, h.project, h.agent).display()
        ));
    }
    s.push_str(&format!("AGENT_ID={}:{}\n", h.project, h.agent));
    s.push_str(&format!("PROJECT_ID={}\n", h.project));
    s.push_str(&format!("RUNTIME={}\n", h.spec.runtime));
    if let Some(m) = h
        .spec
        .model
        .as_ref()
        .or_else(|| runtime.and_then(|r| r.default_model.as_ref()))
    {
        s.push_str(&format!("MODEL={m}\n"));
    }
    if let Some(pm) = &h.spec.permission_mode {
        s.push_str(&format!("PERMISSION_MODE={pm}\n"));
    }
    // T-048: per-agent reasoning effort flows through to the runtime
    // via the wrapper. Workspace-level `.env` `EFFORT=` still wins for
    // operators not yet on the YAML form (back-compat).
    if let Some(effort) = h.spec.effort {
        s.push_str(&format!("EFFORT={}\n", effort.as_str()));
    }
    s.push_str(&format!("TEAMCTL_MAILBOX={}\n", mailbox.display()));
    s.push_str(&format!("MCP_CONFIG={}\n", mcp.display()));
    s.push_str(&format!("SYSTEM_PROMPT_PATH={prompt}\n"));
    s.push_str(&format!(
        "CLAUDE_PROJECT_DIR={}\n",
        project.project.cwd.display()
    ));
    // Absolute path to the compose root (the directory holding
    // `team-compose.yaml`). The wrapper passes this to `teamctl --root`
    // so rl-watch resolves the right tree regardless of where
    // `cd "$CLAUDE_PROJECT_DIR"` lands the shell. Without this,
    // wrapper falls back to CLAUDE_PROJECT_DIR (often a relative `..`)
    // which compounds with the post-cd cwd and points at the wrong
    // directory.
    s.push_str(&format!("TEAMCTL_ROOT={}\n", compose.root.display()));
    s.push_str(&format!(
        "TMUX_SESSION={}{}-{}\n",
        compose.global.supervisor.tmux_prefix, h.project, h.agent
    ));
    // T-118: claude-code agents resume their conversation across
    // teamctl down/up + crash recovery via a deterministic UUIDv5
    // session id. Other runtimes don't recognize `--session-id`, so
    // emit these env vars only for `claude-code` — the wrapper's
    // claude-code arm picks them up; other arms ignore them.
    if h.spec.runtime == "claude-code" {
        let session_id = crate::session::derive_session_id(h.project, h.agent);
        let session_name = crate::session::session_name(h.project, h.agent);
        s.push_str(&format!("CLAUDE_SESSION_ID={session_id}\n"));
        s.push_str(&format!("CLAUDE_SESSION_NAME={session_name}\n"));
        // T-189: path to the wrapper-managed Claude settings file
        // carrying the synchronous-prompt deny hook. Wrapper applies
        // it via `--settings` except when `permission_mode: attended`
        // (human at the keyboard wants the interactive tools back).
        let settings = claude_settings_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_SETTINGS={}\n", settings.display()));
        // #383 Phase 3a: path to the rendered `--agents` JSON carrying this
        // agent's declared sub-agents. Always emitted for claude-code; the
        // file itself is written only when `subagents:` is non-empty, so
        // the wrapper's `[ -f ]` guard decides whether `--agents` is passed.
        let subagents = subagents_json_path(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_AGENTS_JSON={}\n", subagents.display()));
        // #383 Phase 3b: path to the per-agent skills scope dir passed to
        // `claude --add-dir`. Always emitted for claude-code; the dir is
        // materialized only when `skills:` is non-empty, so the wrapper's
        // `[ -d ]` guard decides whether `--add-dir` is passed.
        let scope = agent_scope_dir(&compose.root, h.project, h.agent);
        s.push_str(&format!("CLAUDE_AGENT_SCOPE={}\n", scope.display()));
    }
    // Codex has no `--mcp-config` flag — its MCP servers live in
    // `[mcp_servers.*]` tables inside `$CODEX_HOME/config.toml`, and
    // `CODEX_HOME` relocates codex's whole state root. Point each codex
    // agent at its own rendered home (written by [`write_codex_config`])
    // so the wrapper can export it; other runtimes must not see the var.
    if h.spec.runtime == "codex" {
        let home = codex_home_dir(&compose.root, h.project, h.agent);
        s.push_str(&format!("CODEX_HOME={}\n", home.display()));
    }
    // OpenCode has no `--mcp-config` flag either — servers live in the
    // `mcp` object of the json `OPENCODE_CONFIG` points at (written by
    // [`write_opencode_config`]), and `OPENCODE_DB` relocates the
    // session sqlite db so each agent resumes its own conversation via
    // `-c`. Both are opencode-only; other runtimes must not see them.
    // NOTE: OPENCODE_DB is present in the binary but undocumented
    // upstream — provenance lives in `runtimes/opencode.yaml`.
    if h.spec.runtime == "opencode" {
        let home = opencode_home_dir(&compose.root, h.project, h.agent);
        s.push_str(&format!(
            "OPENCODE_DB={}\n",
            home.join("agent.db").display()
        ));
        s.push_str(&format!(
            "OPENCODE_CONFIG={}\n",
            home.join("opencode.json").display()
        ));
    }
    s
}

/// Materialize the descriptor's argv as shell-quoted tokens. The wrapper
/// adds the vendor-specific flags separately; a custom descriptor can run
/// any executable with static argv and the agent context in its environment.
pub fn write_runtime_launcher(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let runtimes = crate::runtimes::load_all(&compose.root)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")))?;
    let runtime = runtimes
        .get(&h.spec.runtime)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown runtime"))?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut script = format!("#!/bin/sh\nexec {}", quote(&runtime.binary));
    for arg in &runtime.args {
        script.push(' ');
        script.push_str(&quote(arg));
    }
    script.push_str(" \"$@\"\n");
    let path = runtime_launcher_path(&compose.root, h.project, h.agent);
    crate::persistence::atomic_write(&path, script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Resolve the absolute path that `SYSTEM_PROMPT_PATH` will point at.
///
/// - `None` role_prompt → `None` (env line renders as blank).
/// - Single source file → `<root>/<source>` (back-compat, no concat
///   file is written — the operator's source is the prompt).
/// - List form → the materialized concat path under
///   `<root>/state/role_prompts/<project>-<agent>.md`. The file at that
///   path is produced by [`write_role_prompt_concat`]; this helper is
///   pure and only computes the destination.
pub fn system_prompt_path(compose: &Compose, h: AgentHandle<'_>) -> Option<PathBuf> {
    match h.spec.role_prompt.as_ref()? {
        RolePrompt::Single(p) => Some(compose.root.join(p)),
        RolePrompt::Multiple(_) => Some(role_prompt_concat_path(&compose.root, h.project, h.agent)),
    }
}

/// Materialize the multi-file `role_prompt` concatenation for one agent.
///
/// No-op when `role_prompt` is `None` or `Single` — there is nothing to
/// concatenate. For the list form, every source file is read in declared
/// order and joined with [`ROLE_PROMPT_SEPARATOR`]; the result overwrites
/// `<root>/state/role_prompts/<project>-<agent>.md` so subsequent edits
/// to any source file flow into the agent's prompt at the next render.
///
/// Missing source files surface as the underlying `io::Error` so the
/// caller can fail the apply rather than silently emit a partial concat.
pub fn write_role_prompt_concat(compose: &Compose, h: AgentHandle<'_>) -> io::Result<()> {
    let Some(RolePrompt::Multiple(paths)) = h.spec.role_prompt.as_ref() else {
        return Ok(());
    };

    let mut buf = String::new();
    for (idx, rel) in paths.iter().enumerate() {
        if idx > 0 {
            buf.push_str(ROLE_PROMPT_SEPARATOR);
        }
        let abs = compose.root.join(rel);
        let bytes = std::fs::read(&abs).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read role_prompt source {}: {e}", abs.display()),
            )
        })?;
        // Source files are expected to be UTF-8 markdown; lossy decode
        // keeps render diagnostics readable if a stray byte sneaks in.
        buf.push_str(&String::from_utf8_lossy(&bytes));
    }

    let dest = role_prompt_concat_path(&compose.root, h.project, h.agent);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::persistence::atomic_write(&dest, buf.as_bytes())
}
