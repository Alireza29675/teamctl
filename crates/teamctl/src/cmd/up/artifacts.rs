use super::*;
use team_core::render::*;

/// Render only the named project's launch artifacts.
pub fn render_project_public(compose: &Compose, project_id: &str) -> Result<()> {
    render_scope(compose, Some(project_id))
}

/// Render per-agent env + MCP files. Called by `up` and `reload`.
pub fn render_all_public(compose: &Compose) -> Result<()> {
    render_scope(compose, None)
}

fn render_scope(compose: &Compose, project_id: Option<&str>) -> Result<()> {
    let envs_dir = compose.root.join("state/envs");
    let mcp_dir = compose.root.join("state/mcp");
    let claude_dir = compose.root.join("state/claude");
    fs::create_dir_all(&envs_dir)?;
    fs::create_dir_all(&mcp_dir)?;
    fs::create_dir_all(&claude_dir)?;
    // #428: per-agent activity-heartbeat markers (touched by the rendered
    // hooks, stat()d by the TUI). Created here alongside the other state
    // subdirs so the rendered hook can be a bare `touch` of the marker.
    fs::create_dir_all(compose.root.join("state/heartbeats"))?;
    let bin = super::super::team_mcp_bin().display().to_string();
    for h in compose
        .agents()
        .filter(|h| project_id.is_none_or(|id| h.project == id))
    {
        let (env, mcp) = render_agent(compose, h, &bin);
        team_core::persistence::atomic_write(
            &env_path(&compose.root, h.project, h.agent),
            env.as_bytes(),
        )?;
        team_core::persistence::atomic_write(
            &mcp_path(&compose.root, h.project, h.agent),
            mcp.as_bytes(),
        )?;
        if let Some(settings) = render_claude_settings(compose, h) {
            team_core::persistence::atomic_write(
                &claude_settings_path(&compose.root, h.project, h.agent),
                settings.as_bytes(),
            )?;
        }
        // Codex reads MCP servers from `[mcp_servers.*]` tables in its
        // per-agent CODEX_HOME (no --mcp-config flag); render (or clear)
        // that config.toml alongside the JSON.
        write_codex_config(compose, h, &bin)
            .with_context(|| format!("write codex config for {}:{}", h.project, h.agent))?;
        // OpenCode reads MCP servers + instructions from the per-agent
        // opencode.json its OPENCODE_CONFIG points at; render (or clear)
        // it alongside the JSON, same lifecycle as the codex config.
        write_opencode_config(compose, h, &bin)
            .with_context(|| format!("write opencode config for {}:{}", h.project, h.agent))?;
        // Re-materialize multi-file role_prompt concat unconditionally
        // so any edit to a source file flows into the agent's prompt at
        // the next render — single-form is a no-op (back-compat).
        write_role_prompt_concat(compose, h)
            .with_context(|| format!("write role_prompt concat for {}:{}", h.project, h.agent))?;
        // #383 Phase 3a: render the per-agent `--agents` JSON (or clear a
        // stale one) alongside the env/mcp/settings files.
        write_subagents_json(compose, h)
            .with_context(|| format!("write sub-agents json for {}:{}", h.project, h.agent))?;
        // #383 Phase 3b: materialize (or clear) the per-agent skills scope
        // dir so `claude --add-dir` surfaces declared skills.
        write_agent_skills(compose, h)
            .with_context(|| format!("write agent skills for {}:{}", h.project, h.agent))?;
        write_runtime_launcher(compose, h)?;
    }
    Ok(())
}

/// Write `bin/agent-wrapper.sh`, `bin/boot.sh`, and create `state/` subdirs.
///
/// Both scripts are teamctl-managed infrastructure: they get rewritten on
/// every `teamctl up` so upgrading the binary picks up wrapper fixes (pty
/// handling, argv quoting, ...) and boot-context fixes without users having
/// to rm and re-init their workspace. Customization happens through env vars
/// in the generated `state/envs/<agent>.env`, not by editing the scripts.
pub fn ensure_wrapper_and_dirs(compose: &Compose) -> Result<()> {
    write_managed_executable(&super::super::agent_wrapper(&compose.root), DEFAULT_WRAPPER)?;
    write_managed_executable(&boot_script_path(&compose.root), DEFAULT_BOOT_SCRIPT)?;
    fs::create_dir_all(compose.root.join("state/envs"))?;
    fs::create_dir_all(compose.root.join("state/mcp"))?;
    Ok(())
}

/// Write a teamctl-managed executable asset and make it `0o755` on unix.
/// Idempotent: only rewrites when the on-disk copy has drifted from the
/// embedded one, so a `teamctl up` that changes nothing leaves mtimes alone.
pub(super) fn write_managed_executable(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_write = match fs::read_to_string(path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if needs_write {
        team_core::persistence::atomic_write(path, content.as_bytes())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

pub(super) const DEFAULT_WRAPPER: &str = include_str!("../../../assets/agent-wrapper.sh");
pub(super) const DEFAULT_BOOT_SCRIPT: &str = include_str!("../../../assets/boot.sh");
