/// #479: the request body carries exactly `chat_id` (i64) and the raw
/// markdown under `rich_message.markdown` — no block tree, no extra fields.
#[test]
fn rich_message_body_has_chat_and_markdown() {
    assert_eq!(
        rich_message_body(123, "# Hi"),
        serde_json::json!({
            "chat_id": 123,
            "rich_message": { "markdown": "# Hi" },
        }),
    );
}

/// #479: multiline markdown passes through verbatim — Telegram builds the
/// block tree server-side, so the bot must not transform the content.
#[test]
fn rich_message_body_preserves_multiline_markdown() {
    let markdown = "# Heading\n\n| a | b |\n- list\n```\ncode\n```\n> quote";
    let body = rich_message_body(7, markdown);
    assert_eq!(body["rich_message"]["markdown"], markdown);
}

/// #479: an underscore in an agent id would italicize a span in rich
/// markdown; the escape backslashes each special. A schema-clean id like
/// `teamctl:kian` carries no specials and passes through untouched.
#[test]
fn markdown_escape_str_escapes_underscores_and_specials() {
    assert_eq!(markdown_escape_str("team_a:bot_1"), "team\\_a:bot\\_1");
    assert_eq!(markdown_escape_str("teamctl:kian"), "teamctl:kian");
}

/// rich-default-on: the three-way precedence — the `--no-rich-messages`
/// kill switch wins, then a reply target forces the (threading) HTML path,
/// and only a rich-enabled message with no reply target takes the rich path.
#[test]
fn use_rich_path_three_way_precedence() {
    // rich enabled, no reply target → rich (the new default for fresh msgs)
    assert!(use_rich_path(true, false));
    // rich enabled, but a reply target → HTML to preserve threading
    assert!(!use_rich_path(true, true));
    // kill switch wins regardless of reply target
    assert!(!use_rich_path(false, false));
    assert!(!use_rich_path(false, true));
}

/// Parse a `Cli` from the mandatory args plus `extra`, so the CLI-parse
/// tests are hermetic: they pass `--mailbox`/`--token` explicitly and never
/// lean on the dogfood shell's ambient `TEAMCTL_*` env. CI runs with a clean
/// env, so relying on an ambient `TEAMCTL_MAILBOX` is what slipped these
/// tests past local `just test` and failed only in CI.
fn parse_cli(extra: &[&str]) -> Result<Cli, clap::Error> {
    let mut argv = vec![
        "bot",
        "--mailbox",
        "/tmp/teamctl-test-mailbox.db",
        "--token",
        "x",
    ];
    argv.extend_from_slice(extra);
    Cli::try_parse_from(argv)
}

/// rich-default-on: rich messages are ON by default — with no flag the
/// opt-out is unset, so `State.rich` (= `!no_rich_messages`) is true.
#[test]
fn cli_rich_messages_default_on() {
    let cli = parse_cli(&[]).expect("parse");
    assert!(
        !cli.no_rich_messages,
        "rich must be ON by default (opt-out)"
    );
}

/// rich-default-on: `--no-rich-messages` is the opt-out kill switch.
#[test]
fn cli_no_rich_messages_opt_out() {
    let cli = parse_cli(&["--no-rich-messages"]).expect("parse");
    assert!(cli.no_rich_messages, "--no-rich-messages must force HTML");
}

/// rich-default-on: the opt-out accepts boolish values (1/0/true/false/…),
/// not just `true`/`false`. clap applies the arg's `value_parser` to both
/// the env var and the `--flag=value` form, so this `--no-rich-messages=…`
/// test pins the exact parser the `TEAMCTL_TELEGRAM_NO_RICH` env var uses —
/// without mutating process env. A bare `bool` arg would reject `=1` and
/// crash the bot at startup, so this guards the documented kill switch.
#[test]
fn cli_no_rich_messages_accepts_boolish_values() {
    let parse = |v: &str| {
        let arg = format!("--no-rich-messages={v}");
        parse_cli(&[arg.as_str()])
            .unwrap_or_else(|_| panic!("--no-rich-messages={v} must parse"))
            .no_rich_messages
    };
    assert!(
        parse("true"),
        "=true must disable rich (the documented form)"
    );
    assert!(parse("1"), "=1 must disable rich (forgiving boolish form)");
    assert!(!parse("0"), "=0 must keep rich on");
    assert!(!parse("false"), "=false must keep rich on");
}
