#[test]
fn classify_kind_treats_null_and_empty_as_text() {
    // Back-compat pin: rows from before T-086-A migration have NULL
    // kind; rows inserted via legacy `send_dm` still leave it NULL.
    // Both must dispatch as plain text — otherwise older databases
    // would suddenly fail the unknown-kind path.
    assert_eq!(classify_kind(None), DispatchKind::Text);
    assert_eq!(classify_kind(Some("text")), DispatchKind::Text);
    assert_eq!(classify_kind(Some("")), DispatchKind::Text);
}

#[test]
fn classify_kind_routes_image_and_file() {
    assert_eq!(classify_kind(Some("image")), DispatchKind::Image);
    assert_eq!(classify_kind(Some("file")), DispatchKind::File);
}

#[test]
fn classify_kind_falls_back_for_unknown_kinds() {
    // Forward-compat: kinds the binary doesn't recognise surface as
    // a text fallback rather than panicking. T-086-A's prophetic
    // example ("reaction") landed in T-086-E and now routes to its
    // own arm (covered by `classify_kind_routes_reaction`); the
    // fallback test stays useful by pinning truly-unknown strings.
    assert_eq!(
        classify_kind(Some("garbage")),
        DispatchKind::UnknownFallback
    );
    assert_eq!(classify_kind(Some("custom")), DispatchKind::UnknownFallback);
}

#[test]
fn parse_payload_extracts_source_value_and_caption() {
    let p = parse_payload(r#"{"source":"path","value":"/tmp/x.png","caption":"hi"}"#)
        .expect("payload parses");
    assert_eq!(p.source, "path");
    assert_eq!(p.value, "/tmp/x.png");
    assert_eq!(p.caption.as_deref(), Some("hi"));
}

#[test]
fn parse_payload_handles_missing_caption() {
    let p = parse_payload(r#"{"source":"url","value":"https://x.test/a.png"}"#)
        .expect("payload parses");
    assert_eq!(p.source, "url");
    assert!(p.caption.is_none());
}

#[test]
fn parse_payload_returns_none_on_garbage() {
    assert!(parse_payload("not json").is_none());
    assert!(
        parse_payload(r#"{"value":"x"}"#).is_none(),
        "missing source"
    );
    assert!(
        parse_payload(r#"{"source":"path"}"#).is_none(),
        "missing value"
    );
}

#[test]
fn input_file_from_path_and_url_both_construct() {
    // We can't easily assert teloxide internals, but we can pin that
    // both branches return Some() — the negative case (unknown
    // source) is the regression risk and is covered by the next
    // test.
    let p = parse_payload(r#"{"source":"path","value":"/tmp/x.png"}"#).unwrap();
    assert!(input_file_from(&p).is_some());
    let p = parse_payload(r#"{"source":"url","value":"https://x.test/a.png"}"#).unwrap();
    assert!(input_file_from(&p).is_some());
}

#[test]
fn input_file_from_unknown_source_returns_none() {
    let p = MediaPayload {
        source: "bytes".into(),
        value: "abc".into(),
        caption: None,
    };
    assert!(input_file_from(&p).is_none());
}

#[allow(clippy::too_many_arguments)]
fn insert_row(
    conn: &Connection,
    sender: &str,
    text: &str,
    kind: Option<&str>,
    payload: Option<&str>,
    telegram_msg_id: Option<i64>,
) -> i64 {
    let project = sender.split_once(':').map(|(p, _)| p).unwrap_or("p");
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at,
             kind, structured_payload, telegram_msg_id)
         VALUES (?1, ?2, 'user:telegram', ?3, strftime('%s','now'), ?4, ?5, ?6)",
        params![project, sender, text, kind, payload, telegram_msg_id],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// SELECT shape `outbound_loop` runs — kept in sync with the production
/// query so MailboxRow's column ordering stays asserted.
const OUTBOUND_SELECT: &str =
    "SELECT m.id, m.sender, m.text, m.kind, m.structured_payload, m.telegram_msg_id
     FROM messages m
     WHERE m.id > ?1
       AND m.recipient = 'user:telegram'
       AND m.acked_at IS NULL
     ORDER BY m.id";

#[test]
fn outbound_select_returns_kind_and_payload_for_structured_rows() {
    // Pins the SELECT-shape contract: outbound_loop's enriched query
    // surfaces both new columns so the dispatcher can route on them.
    // Without this, a structured row would still be fetched but with
    // text-row defaults — silently degrading image/file to text.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_row(
        &conn,
        "p:eng_lead",
        "shot",
        Some("image"),
        Some(r#"{"source":"path","value":"/tmp/a.png"}"#),
        None,
    );
    let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
    let rows: Vec<MailboxRow> = stmt
        .query_map(params![0i64], MailboxRow::from_row)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].kind.as_deref(), Some("image"));
    assert!(rows[0].payload.as_deref().unwrap().contains("/tmp/a.png"));
}

#[test]
fn outbound_select_returns_null_kind_for_legacy_text_rows() {
    // Pre-T-086-A rows (and rows written by `send_dm`, which leaves
    // kind NULL) still surface in the SELECT — the dispatcher's
    // classify_kind treats NULL as Text, completing the back-compat
    // round-trip.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_row(&conn, "p:eng_lead", "hello", None, None, None);
    let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
    let rows: Vec<MailboxRow> = stmt
        .query_map(params![0i64], MailboxRow::from_row)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert!(rows[0].kind.is_none());
    assert!(rows[0].payload.is_none());
    assert!(rows[0].telegram_msg_id.is_none());
    assert_eq!(classify_kind(rows[0].kind.as_deref()), DispatchKind::Text);
}

#[test]
fn outbound_select_returns_telegram_msg_id_when_set_for_threaded_rows() {
    // T-086-B: outbound rows written with a `reply_to_message_id` carry
    // it forward via `telegram_msg_id`. The dispatcher reads this and
    // attaches `reply_parameters` on send. Pinning the round-trip
    // guards against future SELECT shape regressions silently dropping
    // the threading column.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_row(&conn, "p:eng_lead", "ack", None, None, Some(7777));
    let mut stmt = conn.prepare(OUTBOUND_SELECT).unwrap();
    let rows: Vec<MailboxRow> = stmt
        .query_map(params![0i64], MailboxRow::from_row)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].telegram_msg_id, Some(7777));
}
