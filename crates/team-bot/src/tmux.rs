use std::process::Command;

use teloxide::types::BotCommand;

/// Decision returned by `slash_outcome` — either we have a tmux session to
/// type the slash command into, or a user-facing rejection message.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SlashOutcome {
    Passthrough { session: String },
    Reject { reason: String },
}

/// Pure decision: given the manager id (`<project>:<role>`), the manager's
/// runtime, and the configured tmux prefix, decide whether slash-passthrough
/// fires and against which tmux session. Claude Code, Codex, and OpenCode
/// all run a slash-command REPL that reports unknown commands itself, so
/// all three get passthrough; other runtimes are rejected per Decision 6
/// (manager-only routing) and the rejection message names the actual
/// runtime so the operator sees why.
pub(super) fn slash_outcome(manager: &str, runtime: &str, tmux_prefix: &str) -> SlashOutcome {
    if runtime != "claude-code" && runtime != "codex" && runtime != "opencode" {
        return SlashOutcome::Reject {
            reason: format!(
                "slash-passthrough is only supported on Claude Code, Codex, and \
                 OpenCode agents (this manager runs `{runtime}`)."
            ),
        };
    }
    let (project, role) = match manager.split_once(':') {
        Some((p, r)) => (p, r),
        None => {
            return SlashOutcome::Reject {
                reason: format!("malformed manager id `{manager}` (expected `project:role`)."),
            };
        }
    };
    SlashOutcome::Passthrough {
        session: format!("{tmux_prefix}{project}-{role}"),
    }
}

/// Phase-1 argv: type `body` literally into the target pane. `-l` keeps slash
/// command text such as "Enter" or "C-c" from being interpreted as tmux keys.
pub(super) fn tmux_type_body_argv<'a>(session: &'a str, body: &'a str) -> [&'a str; 5] {
    ["send-keys", "-t", session, "-l", body]
}

/// Phase-2 argv: a separate `Enter` key event that submits the composer.
pub(super) fn tmux_submit_argv(session: &str) -> [&str; 4] {
    ["send-keys", "-t", session, "Enter"]
}

/// Real-world tmux send-keys wrapper. On failure, returns the verbatim error
/// (R12 family — surface the cause to the operator rather than silent drop).
pub(super) fn tmux_send_keys(session: &str, body: &str) -> Result<(), String> {
    let output = Command::new("tmux")
        .args(tmux_type_body_argv(session, body))
        .output()
        .map_err(|e| format!("invoke tmux: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            return Err(format!("tmux exit {}", output.status));
        }
        return Err(format!("tmux exit {}: {trimmed}", output.status));
    }

    // Codex folds body+Enter delivered in one pty write into the composer;
    // a distinct Enter after the TUI renders submits the command cleanly.
    std::thread::sleep(std::time::Duration::from_millis(400));

    let output = Command::new("tmux")
        .args(tmux_submit_argv(session))
        .output()
        .map_err(|e| format!("invoke tmux: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            return Err(format!("tmux exit {}", output.status));
        }
        return Err(format!("tmux exit {}: {trimmed}", output.status));
    }
    Ok(())
}

/// Curated subset of Claude Code slash commands surfaced via Telegram's
/// `setMyCommands` API (T-086-H). Telegram restricts the `command` field to
/// lowercase letters, digits, and underscores — the hyphenated CC commands
/// (`output-style`, `pr-comments`, `release-notes`, `security-review`) are
/// excluded for that reason; operators can still type them manually and the
/// slash-passthrough lane (T-086-G) routes them to tmux just fine. Login
/// flows (`login`, `logout`, `upgrade`) are also excluded — those are
/// awkward over chat and rarely the daily-driver path.
///
/// **Maintenance note**: this list is hand-maintained on Claude Code
/// version bumps. Drift cost is bounded — the CC slash command set is
/// stable across patch releases. The dynamic-discovery alternative (parse
/// CC's `/help` output at startup) is heavier substrate for marginal gain.
/// Refresh in a polish-PR when CC ships a new minor version.
pub(super) const CC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("clear", "Clear conversation history"),
    (
        "compact",
        "Compact conversation, optionally with focus instructions",
    ),
    ("cost", "Show token usage cost"),
    ("help", "Show available commands and shortcuts"),
    ("init", "Initialize a new CLAUDE.md file"),
    ("mcp", "Manage MCP servers"),
    ("model", "Set the AI model for Claude Code"),
    ("permissions", "View and edit permissions"),
    ("resume", "Resume a previous conversation"),
    ("review", "Review a pull request"),
    ("status", "Show Claude Code status"),
    ("vim", "Toggle between vim and default editing modes"),
];

/// Curated subset of Codex CLI slash commands surfaced via Telegram's
/// `setMyCommands` API, mirroring `CC_SLASH_COMMANDS` above. Conservative
/// by construction: only commands verified against the upstream OpenAI
/// docs are listed — extend as further commands are verified rather than
/// guessing at the full set. The same Telegram charset constraint applies
/// (`[a-z0-9_]` only), and the same maintenance note: hand-refresh on
/// Codex CLI version bumps; operators can always type unlisted commands
/// manually and slash-passthrough routes them to tmux just fine.
pub(super) const CODEX_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("agent", "Inspect or switch subagent threads"),
    ("compact", "Compact the conversation context"),
    ("hooks", "Review and trust hooks"),
];

/// Curated subset of OpenCode slash commands surfaced via Telegram's
/// `setMyCommands` API, mirroring the two lists above. Live-captured from
/// the opencode 1.17.13 TUI — extend by verification, not by guessing at
/// the full set. The same Telegram charset constraint applies
/// (`[a-z0-9_]` only), and the same maintenance note: hand-refresh on
/// opencode version bumps; operators can always type unlisted commands
/// manually and slash-passthrough routes them to tmux just fine.
pub(super) const OPENCODE_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("models", "Switch the AI model"),
    ("new", "Start a new session"),
    ("sessions", "List and switch sessions"),
    ("status", "Show opencode status"),
    ("variants", "Cycle the model's reasoning variants"),
];

/// Build the runtime-appropriate `BotCommand` list for `setMyCommands`. CC
/// managers get `CC_SLASH_COMMANDS`, Codex managers get
/// `CODEX_SLASH_COMMANDS`, OpenCode managers get
/// `OPENCODE_SLASH_COMMANDS`; everything else (gemini, unknown, unscoped)
/// gets an empty list — clean degrade per Decision 6 (manager-only
/// routing). Pulled out as a free function so the per-runtime mapping is
/// unit-testable without a real Telegram bot.
pub(super) fn commands_for_runtime(runtime: Option<&str>) -> Vec<BotCommand> {
    match runtime {
        Some("claude-code") => CC_SLASH_COMMANDS
            .iter()
            .map(|(c, d)| BotCommand::new(*c, *d))
            .collect(),
        Some("codex") => CODEX_SLASH_COMMANDS
            .iter()
            .map(|(c, d)| BotCommand::new(*c, *d))
            .collect(),
        Some("opencode") => OPENCODE_SLASH_COMMANDS
            .iter()
            .map(|(c, d)| BotCommand::new(*c, *d))
            .collect(),
        _ => Vec::new(),
    }
}

// ── T-086-C inbound media ───────────────────────────────────────
