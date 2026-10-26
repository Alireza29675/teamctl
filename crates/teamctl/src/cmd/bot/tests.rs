use super::*;

#[test]
fn style_with_emits_ansi_only_when_color_on() {
    // T-181: the wizard's color must honor NO_COLOR / non-TTY via
    // `use_color`. `style_with` is the gate's testable core.
    let colored = style_with("3;90", "note", true);
    assert_eq!(colored, "\x1b[3;90mnote\x1b[0m");

    let plain = style_with("3;90", "note", false);
    assert_eq!(plain, "note");
    assert!(
        !plain.contains('\x1b'),
        "NO_COLOR / non-TTY output must carry no ANSI, got: {plain:?}"
    );
}

#[test]
fn default_token_env_strips_project() {
    assert_eq!(
        default_token_env("teamctl:eng_lead"),
        "TEAMCTL_TG_ENG_LEAD_TOKEN"
    );
    assert_eq!(default_token_env("startup:pm"), "TEAMCTL_TG_PM_TOKEN");
}

#[test]
fn default_chats_env_matches_token_shape() {
    assert_eq!(default_chats_env("p:role-x"), "TEAMCTL_TG_ROLE_X_CHATS");
}

// T-314: token-input masking is display-only — the captured value
// must be byte-identical to what was entered (and identical to
// `prompt`'s capture, so the prompt→prompt_secret swap changes
// only the echo). The echo-off termios path needs a real tty and
// can't be unit-tested; `capture_line` is the value-correctness
// contract that can.
#[test]
fn capture_line_strips_only_the_trailing_newline() {
    assert_eq!(
        capture_line(&b"123456:AAH-abcDEF\n"[..]).unwrap(),
        "123456:AAH-abcDEF"
    );
}

#[test]
fn capture_line_strips_crlf() {
    assert_eq!(
        capture_line(&b"123:tok-_x.Y\r\n"[..]).unwrap(),
        "123:tok-_x.Y"
    );
}

#[test]
fn capture_line_preserves_value_bytes_including_inner_spaces() {
    // Only the line terminator is stripped; inner/edge spaces and
    // token punctuation survive verbatim (the caller applies its
    // own `.trim()` + shape check — capture must not pre-mangle).
    assert_eq!(
        capture_line(&b"  12:AA b:c  \n"[..]).unwrap(),
        "  12:AA b:c  "
    );
}

#[test]
fn capture_line_handles_eof_without_newline() {
    assert_eq!(
        capture_line(&b"123:no-newline"[..]).unwrap(),
        "123:no-newline"
    );
}

#[test]
fn capture_line_matches_prompt_capture_semantics() {
    // Pin parity with `prompt`'s exact trim chain so the swapped
    // field behaves identically on capture.
    let raw = "999:Zz-_.\r\n";
    let via_capture = capture_line(raw.as_bytes()).unwrap();
    let via_prompt_logic = raw
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string();
    assert_eq!(via_capture, via_prompt_logic);
    assert_eq!(via_capture, "999:Zz-_.");
}

#[test]
fn bot_session_name_is_stable_and_unique() {
    assert_eq!(bot_session_name("t-", "teamctl:pm"), "t-bot-teamctl-pm");
    assert_eq!(
        bot_session_name("a-", "startup:eng_lead"),
        "a-bot-startup-eng_lead"
    );
}

#[test]
fn write_env_file_replaces_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    std::fs::write(
        &env_path,
        "EXISTING=value\nTEAMCTL_TG_PM_TOKEN=oldtok\nKEEP=me\n",
    )
    .unwrap();
    write_env_file(
        dir.path(),
        "TEAMCTL_TG_PM_TOKEN",
        "newtok",
        "TEAMCTL_TG_PM_CHATS",
        "12345",
    )
    .unwrap();
    let got = std::fs::read_to_string(&env_path).unwrap();
    assert!(got.contains("EXISTING=value"));
    assert!(got.contains("KEEP=me"));
    assert!(got.contains("TEAMCTL_TG_PM_TOKEN=newtok"));
    assert!(!got.contains("oldtok"));
    assert!(got.contains("TEAMCTL_TG_PM_CHATS=12345"));
}

#[test]
fn upsert_manager_telegram_succeeds_for_consecutive_managers_in_same_project() {
    // T-238 regression: `teamctl bot setup` walks every manager in
    // the compose, and `upsert_manager_telegram` writes the
    // telegram block per manager via `team_core::yaml_edit::save`.
    // The pre-fix project-lookup re-read each candidate file from
    // disk with `serde_yaml::from_str` to match by `project.id`;
    // after the first iteration's write through yaml_edit, a
    // strict re-read of the same file could silently fail
    // (`.unwrap_or(false)` in the find-closure), so the second
    // manager's lookup hit no match and bailed with "project not
    // found in compose" even though both managers lived in the
    // file the loop had just edited.
    //
    // Pins the second-iteration shape: build a minimal `.team/`
    // with two managers in one project, load the compose, call
    // upsert for each in sequence, and assert both calls succeed
    // AND the file ends with both telegram blocks.
    let dir = tempfile::tempdir().unwrap();
    let team = dir.path().join(".team");
    std::fs::create_dir_all(team.join("projects")).unwrap();
    std::fs::create_dir_all(team.join("roles")).unwrap();
    std::fs::write(
        team.join("team-compose.yaml"),
        "version: 2\n\
             broker:\n  type: sqlite\n  path: state/mailbox.db\n\
             supervisor:\n  type: tmux\n  tmux_prefix: a-\n\
             projects:\n  - file: projects/p.yaml\n",
    )
    .unwrap();
    std::fs::write(
        team.join("projects/p.yaml"),
        "version: 2\n\
             project:\n  id: p\n  name: P\n  cwd: .\n\
             managers:\n\
             \x20\x20alpha:\n    runtime: claude-code\n    role_prompt: roles/alpha.md\n\
             \x20\x20beta:\n    runtime: claude-code\n    role_prompt: roles/beta.md\n",
    )
    .unwrap();
    std::fs::write(team.join("roles/alpha.md"), "alpha\n").unwrap();
    std::fs::write(team.join("roles/beta.md"), "beta\n").unwrap();

    let compose = Compose::load(&team).expect("compose loads cleanly");

    upsert_manager_telegram(&compose, "p:alpha", "ALPHA_TOKEN", "ALPHA_CHATS")
        .expect("first manager upsert succeeds");

    upsert_manager_telegram(&compose, "p:beta", "BETA_TOKEN", "BETA_CHATS")
        .expect("second manager upsert in same project must succeed");

    let got = std::fs::read_to_string(team.join("projects/p.yaml")).unwrap();
    assert!(
        got.contains("ALPHA_TOKEN") && got.contains("ALPHA_CHATS"),
        "first manager's telegram block must survive the second write:\n{got}"
    );
    assert!(
        got.contains("BETA_TOKEN") && got.contains("BETA_CHATS"),
        "second manager's telegram block must land:\n{got}"
    );
}

#[test]
fn fresh_essentials_team_bot_setup_yields_resolvable_bridge_spec() {
    // #311 repro. Materialize the REAL shipped `essentials` scaffold
    // (main + ops; `ops:ops` pre-wired with the
    // TEAMCTL_TG_OPS_{TOKEN,CHATS} env-var names), reproduce the
    // two persistence side-effects of `bot setup`'s `wizard_one`
    // for ops:ops (write_env_file + upsert_manager_telegram),
    // reload the compose exactly as `teamctl up` does, and assert
    // the Telegram bridge would receive a usable, resolvable spec.
    //
    // Pins #311's acceptance contract ("fresh init -> bot setup ->
    // round-trip delivers") at the deterministically-testable
    // layer: the persisted config the bridge consumes. The
    // interactive wizard (stdin), Telegram HTTP, and the tmux spawn
    // are out of unit scope — what decides whether the bridge can
    // deliver is exactly the spec + .env this asserts.
    let dir = tempfile::tempdir().unwrap();
    let team = dir.path().join(".team");

    let ess = crate::cmd::init::TEMPLATES
        .iter()
        .find(|t| t.key == "essentials")
        .expect("essentials template present");
    for (rel, content) in ess.entries() {
        let body = content
            .replace("{{project_id}}", "main")
            .replace("{{project_name}}", "Main");
        let path = team.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    // A strict-parse regression on the shipped essentials tree
    // would itself be #311.
    let compose = Compose::load(&team).expect("freshly-scaffolded essentials compose must load");

    assert_eq!(
        all_managers(&compose),
        vec!["ops:ops".to_string()],
        "essentials must expose exactly ops:ops to `bot setup`"
    );

    // The scaffold pre-wires the env-var names; `bot setup` reuses
    // them verbatim (env_names_chosen_by_user == false path).
    let (tok_env, chats_env) = manager_telegram(&compose, "ops:ops")
        .expect("essentials pre-wires ops:ops's telegram env-var names");
    assert_eq!(tok_env, "TEAMCTL_TG_OPS_TOKEN");
    assert_eq!(chats_env, "TEAMCTL_TG_OPS_CHATS");

    write_env_file(&team, &tok_env, "123456:FAKE-TOKEN", &chats_env, "99001122").unwrap();
    upsert_manager_telegram(&compose, "ops:ops", &tok_env, &chats_env).unwrap();

    // Reload exactly as `teamctl up` does, then build bridge specs.
    let compose = Compose::load(&team).expect("compose reloads after bot setup");
    let specs = bot_specs(&compose);
    assert_eq!(specs.len(), 1, "exactly one bot spec (ops:ops) expected");
    let spec = &specs[0];
    assert_eq!(spec.manager, "ops:ops");
    assert_eq!(spec.token_env, "TEAMCTL_TG_OPS_TOKEN");
    assert_eq!(spec.chats_env, "TEAMCTL_TG_OPS_CHATS");

    // up_one() resolves spec.token_env from the sourced .team/.env.
    // File-based assertion (parallel-safe, matching
    // write_env_file_replaces_in_place).
    let env_body = std::fs::read_to_string(team.join(".env")).unwrap();
    assert!(
        env_body.contains("TEAMCTL_TG_OPS_TOKEN=123456:FAKE-TOKEN"),
        "the bot token the bridge resolves must be persisted to .team/.env:\n{env_body}"
    );
    assert!(
        env_body.contains("TEAMCTL_TG_OPS_CHATS=99001122"),
        "the authorized chat id must be persisted to .team/.env:\n{env_body}"
    );

    // The telegram block must round-trip through the typed schema
    // the bridge reads (agent.telegram()) after the yaml_edit write.
    let ops_yaml = std::fs::read_to_string(team.join("projects/ops.yaml")).unwrap();
    let parsed: team_core::compose::Project = serde_yaml::from_str(&ops_yaml).unwrap();
    let tg = parsed
        .managers
        .get("ops")
        .and_then(|a| a.telegram())
        .expect("ops:ops telegram must survive the upsert and re-parse");
    assert_eq!(tg.bot_token_env, "TEAMCTL_TG_OPS_TOKEN");
    assert_eq!(tg.chat_ids_env, "TEAMCTL_TG_OPS_CHATS");
}

#[test]
fn edit_manager_yaml_inserts_interfaces_telegram_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.yaml");
    std::fs::write(
        &path,
        "version: 2\n\
             project:\n  id: p\n  name: P\n  cwd: ..\n\
             managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n",
    )
    .unwrap();
    edit_manager_yaml(&path, "pm", "PM_TOKEN", "PM_CHATS").unwrap();
    let got = std::fs::read_to_string(&path).unwrap();
    assert!(
        got.contains("interfaces:"),
        "missing interfaces block:\n{got}"
    );
    assert!(got.contains("telegram:"));
    assert!(got.contains("bot_token_env: PM_TOKEN"));
    assert!(got.contains("chat_ids_env: PM_CHATS"));

    // Round-trip: parsing should give us the typed struct.
    let parsed: team_core::compose::Project = serde_yaml::from_str(&got).unwrap();
    let tg = parsed
        .managers
        .get("pm")
        .and_then(|a| a.telegram())
        .expect("telegram parses out");
    assert_eq!(tg.bot_token_env, "PM_TOKEN");

    // Idempotent: re-running replaces telegram, doesn't duplicate.
    edit_manager_yaml(&path, "pm", "PM_TOKEN_2", "PM_CHATS_2").unwrap();
    let got2 = std::fs::read_to_string(&path).unwrap();
    assert_eq!(got2.matches("telegram:").count(), 1);
    assert_eq!(got2.matches("interfaces:").count(), 1);
    assert!(got2.contains("PM_TOKEN_2"));
    assert!(!got2.contains("PM_TOKEN\n"));
}

// ── #344 managed-bots: fork + project manager_bot write ──────────

#[test]
fn forced_manual_skips_the_fork() {
    // `--manager <m>` (forced_manual=true) must stay on the manual
    // path without prompting, since managed setup is whole-project.
    assert!(matches!(
        choose_setup_mode(true).unwrap(),
        SetupMode::Manual
    ));
}

fn manager_bot_token_env(yaml: &str) -> String {
    let p: team_core::compose::Project = serde_yaml::from_str(yaml).unwrap();
    p.telegram()
        .and_then(|t| t.manager_bot.as_ref())
        .expect("manager_bot parses out")
        .token_env
        .clone()
}

#[test]
fn splice_manager_bot_seeds_interfaces_when_absent() {
    // Fresh project YAML (no `interfaces:` block) — the splice must
    // seed the top-level key and write the manager_bot token_env so
    // `Project::telegram()` reads it back.
    let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n";
    let doc: team_core::yaml_edit::Document = src.parse().unwrap();
    let got = splice_project_manager_bot(doc, "TEAMCTL_TG_MANAGER_TOKEN")
        .unwrap()
        .to_string();
    assert!(got.contains("interfaces:"), "missing interfaces:\n{got}");
    assert!(got.contains("manager_bot:"), "missing manager_bot:\n{got}");
    assert_eq!(manager_bot_token_env(&got), "TEAMCTL_TG_MANAGER_TOKEN");
    // Seeding must not duplicate the top-level key.
    assert_eq!(
        got.matches("interfaces:").count(),
        1,
        "dup interfaces:\n{got}"
    );
}

#[test]
fn splice_manager_bot_preserves_trailing_comment_on_insert() {
    // #319 deliberate check (insert case): a project YAML ending in a
    // trailing comment must keep that comment after the managed-bots
    // write seeds + splices the interfaces block.
    let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   managers:\n  pm:\n    runtime: claude-code\n    role_prompt: roles/pm.md\n\
                   # keep this trailing comment\n";
    let doc: team_core::yaml_edit::Document = src.parse().unwrap();
    let got = splice_project_manager_bot(doc, "TEAMCTL_TG_MANAGER_TOKEN")
        .unwrap()
        .to_string();
    assert!(
        got.contains("# keep this trailing comment"),
        "trailing comment eaten on insert (#319):\n{got}"
    );
}

#[test]
fn splice_manager_bot_preserves_file_final_comment_on_replace() {
    // #319 deliberate check (the actual trigger): manager_bot.token_env
    // is the file-final content block, followed only by a comment. A
    // force re-run REPLACES that leaf — the trailing comment must
    // survive (the comment-preserving substrate's whole contract).
    let src = "version: 2\n\
                   project:\n  id: p\n  name: P\n  cwd: ..\n\
                   interfaces:\n  telegram:\n    manager_bot:\n      token_env: OLD_TOKEN_ENV\n\
                   # final trailing comment\n";
    let doc: team_core::yaml_edit::Document = src.parse().unwrap();
    let got = splice_project_manager_bot(doc, "NEW_TOKEN_ENV")
        .unwrap()
        .to_string();
    assert_eq!(manager_bot_token_env(&got), "NEW_TOKEN_ENV");
    assert!(
        got.contains("# final trailing comment"),
        "file-final trailing comment eaten on replace (#319 fired for managed-bots path):\n{got}"
    );
}

#[test]
fn managed_bot_write_validates_clean_and_round_trips() {
    // Integration: the managed-bots project write produces a YAML that
    // (1) validates clean through the real `team_core::validate` and
    // (2) round-trips so `Project::telegram().manager_bot` reads back.
    // The interactive wizard (stdin), the Telegram `getMe`/`/start`
    // HTTP, and the managed-bot creation flow are out of unit scope —
    // the managed-bot client is covered by #342's wiremock tests; this
    // pins the persisted schema the rest of the flow composes against.
    let dir = tempfile::tempdir().unwrap();
    let team = dir.path().join(".team");
    let ess = crate::cmd::init::TEMPLATES
        .iter()
        .find(|t| t.key == "essentials")
        .expect("essentials template present");
    for (rel, content) in ess.entries() {
        let body = content
            .replace("{{project_id}}", "main")
            .replace("{{project_name}}", "Main");
        let path = team.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }
    let compose = Compose::load(&team).expect("essentials compose loads");
    let project_id = compose.projects[0].project.id.clone();

    upsert_project_manager_bot(&compose, &project_id, "TEAMCTL_TG_MANAGER_TOKEN")
        .expect("managed_bot write succeeds");

    let compose = Compose::load(&team).expect("compose reloads after managed_bot write");
    let errs = team_core::validate::validate(&compose);
    assert!(
        errs.is_empty(),
        "managed-bots YAML must validate clean: {errs:?}"
    );

    let mb = compose.projects[0]
        .telegram()
        .and_then(|t| t.manager_bot.as_ref())
        .expect("manager_bot round-trips through the schema");
    assert_eq!(mb.token_env, "TEAMCTL_TG_MANAGER_TOKEN");
}

// ── #385: resume_prompt keys on whether .env HAS values ──────────
//
// The bug this fixes: the wording must branch on whether `.env`
// actually holds the token / chat id (`token_set` / `chats_set`),
// not on whether the template merely declares the env-var names.
// Declaring names alone is first-time setup, not a re-run. Each arm
// is pinned by a distinctive substring rather than the whole string,
// so wording tweaks that preserve intent don't break the test.

#[test]
fn resume_prompt_nothing_set_reads_as_first_time_setup() {
    let msg = resume_prompt("p:pm", "TOK_ENV", "CHATS_ENV", false, false);
    assert!(
        msg.contains("Set up Telegram bot for"),
        "first-time wording expected:\n{msg}"
    );
    assert!(msg.contains("p:pm"), "manager name expected:\n{msg}");
}

#[test]
fn resume_prompt_token_only_collects_chat_id() {
    let msg = resume_prompt("p:pm", "TOK_ENV", "CHATS_ENV", true, false);
    assert!(
        msg.contains("Token already in"),
        "token-present wording expected:\n{msg}"
    );
    assert!(
        msg.contains("TOK_ENV"),
        "the token env-var name expected:\n{msg}"
    );
    assert!(
        msg.contains("collect the chat id"),
        "should promise to collect the missing chat id:\n{msg}"
    );
}

#[test]
fn resume_prompt_chats_only_collects_token() {
    let msg = resume_prompt("p:pm", "TOK_ENV", "CHATS_ENV", false, true);
    assert!(
        msg.contains("Chat id already in"),
        "chat-id-present wording expected:\n{msg}"
    );
    assert!(
        msg.contains("CHATS_ENV"),
        "the chats env-var name expected:\n{msg}"
    );
    assert!(
        msg.contains("collect the token"),
        "should promise to collect the missing token:\n{msg}"
    );
}

#[test]
fn resume_prompt_both_set_reads_as_re_run() {
    // Only reachable under --force (the no-force fully-wired case
    // early-returns in wizard_one).
    let msg = resume_prompt("p:pm", "TOK_ENV", "CHATS_ENV", true, true);
    assert!(
        msg.contains("Re-run Telegram setup"),
        "re-run wording expected:\n{msg}"
    );
    assert!(
        msg.contains("Existing env-var names"),
        "should note the existing env-var names are reused:\n{msg}"
    );
}

// ── T-314: process_secret_bytes decodes the masked reader's stream ─
//
// Mirrors the per-byte rules of the echo-off termios reader (which
// needs a real tty and can't be unit-tested directly). The masking
// is display-only — the decoded value must match what was entered.

#[test]
fn process_secret_bytes_keeps_plain_ascii_verbatim() {
    assert_eq!(
        process_secret_bytes(b"123456:abcDEF").unwrap(),
        "123456:abcDEF"
    );
}

#[test]
fn process_secret_bytes_terminates_on_cr_or_lf() {
    // CR/LF ends input; nothing before it is lost.
    assert_eq!(process_secret_bytes(b"abc\r").unwrap(), "abc");
    assert_eq!(process_secret_bytes(b"abc\n").unwrap(), "abc");
}

#[test]
fn process_secret_bytes_ignores_bytes_after_terminator() {
    // A CR ends the read; any trailing bytes (e.g. the LF of a CRLF,
    // or a second pasted line) are dropped.
    assert_eq!(process_secret_bytes(b"abc\rxyz").unwrap(), "abc");
}

#[test]
fn process_secret_bytes_backspace_clears_whole_buffer() {
    // Backspace / Delete wipes the ENTIRE entry (re-pasting beats
    // editing a hidden secret), so only what follows survives. Both
    // DEL (0x7f) and BS (0x08) behave identically.
    assert_eq!(process_secret_bytes(b"abc\x7fxyz").unwrap(), "xyz");
    assert_eq!(process_secret_bytes(b"abc\x08xyz").unwrap(), "xyz");
}

#[test]
fn process_secret_bytes_drops_other_control_bytes() {
    // Non-printable control bytes (SOH, ESC, …) draw nothing and are
    // dropped from the value; the printable bytes around them survive.
    assert_eq!(process_secret_bytes(b"a\x01b\x1bc").unwrap(), "abc");
}

#[test]
fn process_secret_bytes_round_trips_pasted_multibyte_utf8() {
    // A pasted token with multibyte UTF-8 (accented letters + a
    // check mark) must survive byte-for-byte — the survivors are
    // decoded once as UTF-8, not per-byte.
    let original = "tökèn-✓-naïve";
    assert_eq!(process_secret_bytes(original.as_bytes()).unwrap(), original);
}

#[test]
fn process_secret_bytes_empty_input_yields_empty_string() {
    assert_eq!(process_secret_bytes(b"").unwrap(), "");
}

#[test]
fn process_secret_bytes_rejects_invalid_utf8() {
    // A lone 0xff is not valid UTF-8; decoding must surface an error
    // rather than silently producing a lossy / corrupt token.
    assert!(process_secret_bytes(&[b'a', 0xff]).is_err());
}

#[test]
fn named_telegram_instances_for_one_agent_have_independent_sessions_and_ids() {
    let (_dir, mut compose) = crate::cmd::up::regression_tests::fixture();
    compose.global.interfaces = serde_yaml::from_str(
        r#"
- id: home-bot
  provider: telegram
  name: Home
  description: Personal requests
  config: {agent: 'p:a', bot_token_env: HOME_TOKEN, authorized_chat_ids_env: HOME_CHATS}
- id: work-bot
  provider: telegram
  name: Work
  description: Work requests
  config: {agent: 'p:a', bot_token_env: WORK_TOKEN, authorized_chat_ids_env: WORK_CHATS}
"#,
    )
    .unwrap();
    let specs = bot_specs(&compose);
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].manager, "p:a");
    assert_eq!(specs[1].manager, "p:a");
    assert_ne!(specs[0].session, specs[1].session);
    assert_eq!(specs[0].identity(), "p:a@home-bot");
    assert_eq!(specs[1].identity(), "p:a@work-bot");
    assert_eq!(
        specs[1].interface_description.as_deref(),
        Some("Work requests")
    );
}
