use serde_json::{json, Value};

use super::{content_json, Ctx};

/// T-109: deliver `/compact` to the calling agent's own tmux pane via
/// `tmux send-keys`. Open to any agent (workers and managers) on
/// runtimes whose TUI ships a `/compact` slash command — Claude Code and
/// Codex both do, with identical semantics. The gate exists for runtimes
/// without a known compact command (gemini has no verified equivalent);
/// sending `/compact` there would just land as input. The destructive
/// warning lives in the schema description, role instructions decide
/// when to call it. Fire-and-forget: the MCP call returns immediately;
/// the slash command lands in the agent's pane on the blocking pool.
/// Detachable — when the runtimes ship native agent-driven compaction,
/// this whole handler + its registry entries delete in one PR.
pub(super) async fn compact_self(ctx: &Ctx) -> Result<Value, String> {
    if let Some(context) = &ctx.attachments {
        let compose = team_core::compose::Compose::load_read_only(&context.compose_root)
            .map_err(|error| format!("compact_self: cannot determine execution mode: {error}"))?;
        let native = compose
            .agents()
            .find(|agent| agent.id() == ctx.agent_id)
            .is_some_and(|agent| {
                agent.spec.execution.as_ref().is_some_and(|execution| {
                    execution.mode == team_core::compose::ExecutionMode::Native
                })
            });
        if native {
            return Err("compact_self is unavailable for native execution; the provider manages context compaction automatically. Terminal /compact requires execution.mode: tmux.".into());
        }
    }
    let runtime = ctx
        .store
        .runtime_for(&ctx.agent_id)
        .map_err(|e| e.to_string())?;
    if !matches!(runtime.as_deref(), Some("claude-code") | Some("codex")) {
        return Err(format!(
            "compact_self: /compact is only supported on Claude Code and Codex agents \
             (caller={} runs `{}`)",
            ctx.agent_id,
            runtime.as_deref().unwrap_or("unknown"),
        ));
    }
    let session = pane_session(&ctx.tmux_prefix, &ctx.agent_id)?;
    // Fire-and-forget. Run the blocking tmux invoke on the blocking pool
    // and drop the join handle so the MCP response goes back without
    // waiting for tmux. On failure we log and drop — the caller deliberately
    // doesn't observe send-keys errors (that's the point of fire-and-forget).
    let session_for_spawn = session.clone();
    tokio::task::spawn_blocking(move || {
        tmux_type_and_submit(&session_for_spawn, "/compact", "compact_self");
    });
    Ok(content_json(&json!({
        "status": "dispatched",
        "session": session,
    })))
}

/// Phase-1 argv: type `line` into a pane literally. `-l` disables tmux
/// key-name lookup so a message body containing "Enter"/"C-c"/";" is sent
/// as text, not interpreted as keys; the single-argv form (no shell)
/// keeps arbitrary bytes injection-safe. Pulled out so tests pin the
/// shape without spinning up tmux.
pub(super) fn type_line_argv<'a>(session: &'a str, line: &'a str) -> [&'a str; 5] {
    ["send-keys", "-t", session, "-l", line]
}

/// Phase-2 argv: a bare `Enter` key event that submits the composer.
pub(super) fn submit_argv(session: &str) -> [&str; 4] {
    ["send-keys", "-t", session, "Enter"]
}

/// Type a line into a tmux pane and submit it, as two separate
/// `send-keys` invocations with a beat between them.
///
/// Why split, not `send-keys "<line>" Enter`: codex's line editor folds a
/// `\r` that arrives in the *same* pty write as the text into the
/// composer (a soft newline) instead of submitting — the note then
/// strands in the input box until a human presses Enter. A distinct Enter
/// key event, after the TUI has consumed the text on its next render
/// tick, submits cleanly. Verified live against codex 0.144.3 (needs the
/// split) and opencode 1.17 (submits either way, so the split is safe for
/// both). Blocking (one short sleep); callers run it on the blocking
/// pool. `label` names the caller in the warn line.
pub(crate) fn tmux_type_and_submit(session: &str, line: &str, label: &str) {
    let type_out = std::process::Command::new("tmux")
        .args(type_line_argv(session, line))
        .output();
    match type_out {
        Ok(o) if !o.status.success() => {
            tracing::warn!(
                session = %session,
                error = %String::from_utf8_lossy(&o.stderr).trim(),
                "{label}: tmux type failed",
            );
            return;
        }
        Err(e) => {
            tracing::warn!(session = %session, error = %e, "{label}: tmux invoke failed");
            return;
        }
        _ => {}
    }
    // Let the TUI render and reset its input state before the submit; a
    // same-tick `\r` is what codex folds into the composer.
    std::thread::sleep(std::time::Duration::from_millis(400));
    if let Err(e) = std::process::Command::new("tmux")
        .args(submit_argv(session))
        .output()
    {
        tracing::warn!(session = %session, error = %e, "{label}: tmux submit failed");
    }
}

#[cfg(test)]
pub(super) fn compact_self_argv(session: &str) -> [&str; 5] {
    type_line_argv(session, "/compact")
}

/// Compose the tmux session name for `agent_id` under `tmux_prefix`,
/// matching the supervisor's canonical formula
/// (`{tmux_prefix}{project}-{role}`). Shared by `compact_self` and the
/// channel watcher's non-claude nudge path (`main.rs`), which both
/// deliver keystrokes into the agent's own pane.
pub(crate) fn pane_session(tmux_prefix: &str, agent_id: &str) -> Result<String, String> {
    let (project, role) = agent_id
        .split_once(':')
        .ok_or_else(|| format!("malformed agent id `{agent_id}` (expected `project:role`)"))?;
    Ok(format!("{tmux_prefix}{project}-{role}"))
}
