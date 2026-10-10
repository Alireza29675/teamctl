#[test]
fn slash_outcome_passes_through_for_claude_code_runtime() {
    let outcome = slash_outcome("writing:manager", "claude-code", "t-");
    assert_eq!(
        outcome,
        SlashOutcome::Passthrough {
            session: "t-writing-manager".into(),
        }
    );
}

#[test]
fn slash_outcome_honours_custom_tmux_prefix() {
    // `compose.global.supervisor.tmux_prefix` is operator-configurable.
    // The session formatter must concatenate verbatim — no hidden
    // dash-or-anything between prefix and project segment.
    let outcome = slash_outcome("news:head_editor", "claude-code", "a-");
    assert_eq!(
        outcome,
        SlashOutcome::Passthrough {
            session: "a-news-head_editor".into(),
        }
    );
}

#[test]
fn slash_outcome_passes_through_for_codex_runtime() {
    // Codex's TUI is a slash-command REPL like Claude Code's and
    // reports unknown commands itself, so passthrough parity is safe.
    let outcome = slash_outcome("writing:manager", "codex", "t-");
    assert_eq!(
        outcome,
        SlashOutcome::Passthrough {
            session: "t-writing-manager".into(),
        }
    );
}

#[test]
fn slash_outcome_passes_through_for_opencode_runtime() {
    // OpenCode's TUI is a slash-command REPL like Claude Code's and
    // Codex's and reports unknown commands itself, so passthrough
    // parity is safe.
    let outcome = slash_outcome("writing:manager", "opencode", "t-");
    assert_eq!(
        outcome,
        SlashOutcome::Passthrough {
            session: "t-writing-manager".into(),
        }
    );
}

#[test]
fn slash_outcome_rejects_gemini_runtime_with_named_runtime() {
    let outcome = slash_outcome("writing:manager", "gemini", "t-");
    let SlashOutcome::Reject { reason } = outcome else {
        panic!("non-CC runtime must reject");
    };
    assert!(reason.contains("gemini"), "names the runtime: {reason}");
}

#[test]
fn slash_outcome_rejects_malformed_manager_id() {
    // Defence in depth: if state.manager somehow lost the `:` (CLI
    // misuse, hand-edited env), refuse to type into a session
    // computed from a half-id rather than guess.
    let outcome = slash_outcome("not-a-manager-id", "claude-code", "t-");
    let SlashOutcome::Reject { reason } = outcome else {
        panic!("malformed manager id must reject");
    };
    assert!(reason.contains("malformed"), "names the failure: {reason}");
}

#[test]
fn tmux_type_body_argv_pins_literal_body_shape() {
    // `-l` makes the slash command body literal text, not tmux key names.
    let argv = tmux_type_body_argv("t-writing-manager", "/clear");
    assert_eq!(
        argv,
        ["send-keys", "-t", "t-writing-manager", "-l", "/clear"]
    );
}

#[test]
fn tmux_submit_argv_pins_separate_enter_shape() {
    // Codex strands slash commands when body and Enter land in one write;
    // keep the submit as its own tmux invocation.
    assert_eq!(
        tmux_submit_argv("t-writing-manager"),
        ["send-keys", "-t", "t-writing-manager", "Enter"]
    );
}

#[test]
fn tmux_type_body_argv_passes_body_verbatim_no_quote_munging() {
    // `Command::args` doesn't shell-quote — argv positions are passed
    // straight through. Tests pin that bodies with spaces / quotes
    // travel as a single arg without our code adding quoting that
    // tmux would then take literally.
    let argv = tmux_type_body_argv("sess", "/compact focus on the cascade");
    assert_eq!(argv[4], "/compact focus on the cascade");
}

#[test]
fn tmux_type_body_argv_keeps_key_names_literal() {
    let argv = tmux_type_body_argv("sess", "/note press Enter then C-c");
    assert_eq!(
        argv,
        [
            "send-keys",
            "-t",
            "sess",
            "-l",
            "/note press Enter then C-c"
        ]
    );
}

// ── T-086-H setMyCommands registration ────────────────────────

#[test]
fn commands_for_runtime_returns_full_cc_list_for_claude_code() {
    let cmds = commands_for_runtime(Some("claude-code"));
    assert_eq!(
        cmds.len(),
        CC_SLASH_COMMANDS.len(),
        "CC manager registers the full curated list"
    );
    let names: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
    // Spot-check a few representative entries — adding/removing
    // entries from CC_SLASH_COMMANDS should consciously update
    // these spot-checks rather than silently drift.
    assert!(names.contains(&"clear"), "must include /clear: {names:?}");
    assert!(
        names.contains(&"compact"),
        "must include /compact: {names:?}"
    );
    assert!(names.contains(&"help"), "must include /help: {names:?}");
}

#[test]
fn commands_for_runtime_returns_full_codex_list_for_codex() {
    let cmds = commands_for_runtime(Some("codex"));
    assert_eq!(
        cmds.len(),
        CODEX_SLASH_COMMANDS.len(),
        "codex manager registers the full curated list"
    );
    // Pin the exact set — order and descriptions included — so an
    // edit to CODEX_SLASH_COMMANDS shows up here consciously.
    for (cmd, (name, desc)) in cmds.iter().zip(CODEX_SLASH_COMMANDS) {
        assert_eq!(cmd.command, *name);
        assert_eq!(cmd.description, *desc);
    }
}

#[test]
fn commands_for_runtime_returns_full_opencode_list_for_opencode() {
    let cmds = commands_for_runtime(Some("opencode"));
    assert_eq!(
        cmds.len(),
        OPENCODE_SLASH_COMMANDS.len(),
        "opencode manager registers the full curated list"
    );
    // Pin the exact set — order and descriptions included — so an
    // edit to OPENCODE_SLASH_COMMANDS shows up here consciously.
    for (cmd, (name, desc)) in cmds.iter().zip(OPENCODE_SLASH_COMMANDS) {
        assert_eq!(cmd.command, *name);
        assert_eq!(cmd.description, *desc);
    }
}

#[test]
fn commands_for_runtime_returns_empty_for_gemini() {
    assert!(commands_for_runtime(Some("gemini")).is_empty());
}

#[test]
fn commands_for_runtime_returns_empty_for_unknown_runtime() {
    // Forward-compat: a future runtime ships before its
    // command-list does. The empty-list fallback means an old
    // team-bot binary against a new runtime degrades quietly.
    assert!(commands_for_runtime(Some("a-future-runtime")).is_empty());
}

#[test]
fn commands_for_runtime_returns_empty_for_unscoped_bot() {
    // Unscoped bot (no `--manager`) → no runtime → no commands.
    // Slash-passthrough is gated on `state.manager.is_some()`
    // anyway, so the autocomplete would be misleading without it.
    assert!(commands_for_runtime(None).is_empty());
}

#[test]
fn cc_slash_command_names_satisfy_telegram_constraints() {
    // Telegram restricts `BotCommand.command` to 1-32 chars,
    // lowercase letters / digits / underscores only. Pinning here
    // so a future CC slash-command-set update that adds a hyphen
    // (e.g. `output-style`) trips this test before hitting the
    // Telegram API and getting silently rejected.
    for (cmd, _desc) in CC_SLASH_COMMANDS {
        assert!(
            !cmd.is_empty() && cmd.len() <= 32,
            "command `{cmd}` violates 1-32 char limit"
        );
        assert!(
            cmd.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "command `{cmd}` contains chars Telegram rejects (only [a-z0-9_])"
        );
    }
}

#[test]
fn cc_slash_command_descriptions_satisfy_telegram_constraints() {
    // Telegram requires `BotCommand.description` to be 3-256 chars.
    // Pinning here for the same reason as the command-name test.
    for (cmd, desc) in CC_SLASH_COMMANDS {
        assert!(
            desc.len() >= 3 && desc.len() <= 256,
            "description for `{cmd}` violates 3-256 char limit (got {} chars: {desc:?})",
            desc.len()
        );
    }
}

#[test]
fn codex_slash_command_names_satisfy_telegram_constraints() {
    // Same Telegram `[a-z0-9_]` / 1-32 char constraint as the CC list
    // — trips here before a future addition hits the API and gets
    // silently rejected.
    for (cmd, _desc) in CODEX_SLASH_COMMANDS {
        assert!(
            !cmd.is_empty() && cmd.len() <= 32,
            "command `{cmd}` violates 1-32 char limit"
        );
        assert!(
            cmd.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "command `{cmd}` contains chars Telegram rejects (only [a-z0-9_])"
        );
    }
}

#[test]
fn codex_slash_command_descriptions_satisfy_telegram_constraints() {
    // Telegram requires `BotCommand.description` to be 3-256 chars.
    for (cmd, desc) in CODEX_SLASH_COMMANDS {
        assert!(
            desc.len() >= 3 && desc.len() <= 256,
            "description for `{cmd}` violates 3-256 char limit (got {} chars: {desc:?})",
            desc.len()
        );
    }
}

#[test]
fn opencode_slash_command_names_satisfy_telegram_constraints() {
    // Same Telegram `[a-z0-9_]` / 1-32 char constraint as the CC and
    // codex lists — trips here before a future addition hits the API
    // and gets silently rejected.
    for (cmd, _desc) in OPENCODE_SLASH_COMMANDS {
        assert!(
            !cmd.is_empty() && cmd.len() <= 32,
            "command `{cmd}` violates 1-32 char limit"
        );
        assert!(
            cmd.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "command `{cmd}` contains chars Telegram rejects (only [a-z0-9_])"
        );
    }
}

#[test]
fn opencode_slash_command_descriptions_satisfy_telegram_constraints() {
    // Telegram requires `BotCommand.description` to be 3-256 chars.
    for (cmd, desc) in OPENCODE_SLASH_COMMANDS {
        assert!(
            desc.len() >= 3 && desc.len() <= 256,
            "description for `{cmd}` violates 3-256 char limit (got {} chars: {desc:?})",
            desc.len()
        );
    }
}

// ── T-086-E reaction dispatch ────────────────────────────────
