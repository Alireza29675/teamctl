/// T-104: `/readnow ` is the case-sensitive, single-space-separated
/// prefix that flips a Telegram-routed message to immediate
/// (full-body inline) delivery. Anything else is a regular lazy row.
#[test]
fn peel_readnow_strips_prefix_when_present() {
    assert_eq!(
        peel_readnow("/readnow build broke"),
        ("build broke", Some("immediate")),
    );
}

#[test]
fn peel_readnow_passes_through_when_prefix_absent() {
    assert_eq!(peel_readnow("regular message"), ("regular message", None));
}

#[test]
fn peel_readnow_is_case_sensitive() {
    // `/ReadNow` and `/READNOW` are not the literal prefix — pass
    // through. Avoids surprising lowercase-vs-mixed-case false hits.
    assert_eq!(peel_readnow("/ReadNow x"), ("/ReadNow x", None));
    assert_eq!(peel_readnow("/READNOW x"), ("/READNOW x", None));
}

#[test]
fn peel_readnow_requires_single_space_separator() {
    // `/readnowfoo` is not the prefix; `/readnow  x` (double space)
    // strips only the first space and the second space stays in body.
    assert_eq!(peel_readnow("/readnowfoo"), ("/readnowfoo", None));
    assert_eq!(peel_readnow("/readnow  x"), (" x", Some("immediate")));
}

#[test]
fn peel_readnow_with_empty_body_after_prefix() {
    // Operator typed only `/readnow ` — preserve the empty body so
    // the caller can reject (sending an empty immediate row would be
    // useless). Caller is responsible for the empty-body guard.
    assert_eq!(peel_readnow("/readnow "), ("", Some("immediate")));
}

// ── #334 inbound reply / quote context ───────────────────────

#[test]
fn truncate_quote_passes_short_text_through() {
    // Under the budget: returned verbatim, no ellipsis.
    assert_eq!(truncate_quote("hello"), "hello");
    let exactly = "a".repeat(INBOUND_QUOTE_MAX_CHARS);
    assert_eq!(
        truncate_quote(&exactly),
        exactly,
        "exactly at the cap is untouched"
    );
}

#[test]
fn truncate_quote_cuts_long_text_with_ellipsis() {
    let long = "a".repeat(INBOUND_QUOTE_MAX_CHARS + 100);
    let out = truncate_quote(&long);
    // Kept chars + one ellipsis char.
    assert_eq!(out.chars().count(), INBOUND_QUOTE_MAX_CHARS + 1);
    assert!(out.ends_with('…'));
    assert!(out.starts_with(&"a".repeat(INBOUND_QUOTE_MAX_CHARS)));
}

#[test]
fn truncate_quote_counts_chars_not_bytes() {
    // Multibyte input must truncate on a char boundary (no panic, no
    // split codepoint). Each `é` is 2 bytes.
    let multibyte = "é".repeat(INBOUND_QUOTE_MAX_CHARS + 50);
    let out = truncate_quote(&multibyte);
    assert_eq!(out.chars().count(), INBOUND_QUOTE_MAX_CHARS + 1);
    assert!(out.ends_with('…'));
}

#[test]
fn build_inbound_body_passes_plain_message_through() {
    // No reply context: the body is forwarded byte-for-byte.
    assert_eq!(build_inbound_body("just a message", None), "just a message");
}

#[test]
fn build_inbound_body_prefixes_reply_with_mailbox_id() {
    let ctx = ReplyContext {
        sender: "user:telegram".to_string(),
        mailbox_id: Some(730),
        quoted: Some("handle the auth case".to_string()),
    };
    assert_eq!(
        build_inbound_body("yes please", Some(&ctx)),
        "> [reply to user:telegram msg 730]\n> handle the auth case\n\nyes please"
    );
}

#[test]
fn build_inbound_body_omits_msg_id_when_unresolved() {
    // Replied-to message not in the mailbox: header carries the sender
    // but no `msg N`, and the quoted text still surfaces.
    let ctx = ReplyContext {
        sender: "agent".to_string(),
        mailbox_id: None,
        quoted: Some("earlier note".to_string()),
    };
    assert_eq!(
        build_inbound_body("ok", Some(&ctx)),
        "> [reply to agent]\n> earlier note\n\nok"
    );
}

#[test]
fn build_inbound_body_blockquotes_each_line_of_a_multiline_quote() {
    let ctx = ReplyContext {
        sender: "p:eng_lead".to_string(),
        mailbox_id: Some(12),
        quoted: Some("line one\nline two".to_string()),
    };
    assert_eq!(
        build_inbound_body("got it", Some(&ctx)),
        "> [reply to p:eng_lead msg 12]\n> line one\n> line two\n\ngot it"
    );
}

#[test]
fn build_inbound_body_header_only_when_nothing_quotable() {
    // A reply to a message with no text/caption (e.g. a sticker): the
    // header still records who/which message, with no quoted lines.
    let ctx = ReplyContext {
        sender: "user:telegram".to_string(),
        mailbox_id: Some(5),
        quoted: None,
    };
    assert_eq!(
        build_inbound_body("what about this", Some(&ctx)),
        "> [reply to user:telegram msg 5]\n\nwhat about this"
    );
}

#[test]
fn lookup_replied_row_finds_row_by_telegram_id() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // A user inbound row and an agent outbound row, each carrying its
    // Telegram message id — both must be reverse-lookupable.
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p','user:telegram','p:eng_lead','hi',0,730)",
        [],
    )
    .unwrap();
    let user_row = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p','p:eng_lead','user:telegram','on it',0,731)",
        [],
    )
    .unwrap();
    let agent_row = conn.last_insert_rowid();

    assert_eq!(
        lookup_replied_row(&conn, 730),
        Some((user_row, "user:telegram".to_string())),
        "a user's own message resolves to its mailbox row + sender"
    );
    assert_eq!(
        lookup_replied_row(&conn, 731),
        Some((agent_row, "p:eng_lead".to_string())),
        "an agent's outbound message resolves to its <project>:<agent> sender"
    );
}

#[test]
fn lookup_replied_row_returns_none_when_absent() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // A Telegram id with no matching row (older than the bot's history).
    assert_eq!(lookup_replied_row(&conn, 999), None);
}

#[test]
fn lookup_replied_row_picks_most_recent_on_telegram_id_collision() {
    // Telegram ids are per-chat, so the shared mailbox can hold two rows
    // with the same telegram_msg_id from different chats. The `ORDER BY id
    // DESC LIMIT 1` is load-bearing: it resolves to the most recently
    // inserted row. Pin that so a refactor can't silently drop the ORDER BY.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p','user:telegram','p:eng_lead','first',0,42)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at, telegram_msg_id)
         VALUES ('p','p:eng_lead','user:telegram','second',0,42)",
        [],
    )
    .unwrap();
    let newer = conn.last_insert_rowid();
    assert_eq!(
        lookup_replied_row(&conn, 42),
        Some((newer, "p:eng_lead".to_string())),
        "collision resolves to the most recently inserted row"
    );
}

#[test]
fn sender_label_for_classifies_bot_human_and_anonymous() {
    assert_eq!(sender_label_for(Some(true)), "agent");
    assert_eq!(sender_label_for(Some(false)), "user:telegram");
    assert_eq!(sender_label_for(None), "unknown");
}

#[test]
fn build_inbound_body_renders_quote_only_reply() {
    // Telegram's hold-to-quote without a full reply-tap: `quote` is present
    // but there's no resolvable replied-to row, so the sender is `unknown`
    // and the mailbox id is omitted. The user's selection still surfaces.
    let ctx = ReplyContext {
        sender: "unknown".to_string(),
        mailbox_id: None,
        quoted: Some("the part I mean".to_string()),
    };
    assert_eq!(
        build_inbound_body("clarify this", Some(&ctx)),
        "> [reply to unknown]\n> the part I mean\n\nclarify this"
    );
}
