#[test]
fn extension_from_mime_covers_canonical_types() {
    assert_eq!(extension_from_mime("image/png"), "png");
    assert_eq!(extension_from_mime("image/jpeg"), "jpg");
    assert_eq!(extension_from_mime("image/webp"), "webp");
    assert_eq!(extension_from_mime("image/gif"), "gif");
    assert_eq!(extension_from_mime("application/pdf"), "pdf");
    assert_eq!(extension_from_mime("text/plain"), "txt");
    assert_eq!(extension_from_mime("application/zip"), "zip");
}

#[test]
fn extension_from_mime_falls_back_to_bin_for_unknown() {
    // Forward-compat: a mime we haven't mapped still produces a real
    // file with a non-empty extension. The agent can re-mime via
    // libmagic if it cares; we don't pretend we know.
    assert_eq!(extension_from_mime("application/octet-stream"), "bin");
    assert_eq!(extension_from_mime("video/mp4"), "bin");
    assert_eq!(extension_from_mime(""), "bin");
}

#[test]
fn extension_for_document_prefers_filename_extension() {
    // Filename's tail wins when it's a clean alphanumeric suffix —
    // even when it disagrees with the upload's mime type. Telegram
    // operators sometimes mislabel; trust the filename they typed.
    assert_eq!(
        extension_for_document(Some("report.pdf"), "application/octet-stream"),
        "pdf"
    );
    assert_eq!(
        extension_for_document(Some("snapshot.PNG"), "application/pdf"),
        "png",
        "case-folded to lowercase"
    );
}

#[test]
fn extension_for_document_falls_back_to_mime_when_filename_missing() {
    assert_eq!(extension_for_document(None, "image/png"), "png");
    assert_eq!(
        extension_for_document(None, "application/octet-stream"),
        "bin"
    );
}

#[test]
fn extension_for_document_rejects_funky_extensions() {
    // No extension → mime fallback.
    assert_eq!(extension_for_document(Some("README"), "text/plain"), "txt");
    // Empty extension after dot → mime fallback.
    assert_eq!(
        extension_for_document(Some("trailing."), "text/plain"),
        "txt"
    );
    // Non-alphanumeric chars → mime fallback (defends against weird
    // shell/fs metacharacters that could foul a path).
    assert_eq!(
        extension_for_document(Some("name.weird/ext"), "image/png"),
        "png"
    );
    // Over-long extension → mime fallback (sanity cap on what we'd
    // accept verbatim from a user-controlled filename).
    assert_eq!(
        extension_for_document(Some("name.thisistoolongatail"), "image/png"),
        "png"
    );
}

#[test]
fn inbound_media_path_composes_root_project_rowid_extension() {
    let root = std::path::Path::new("/srv/.team/state/inbound-media");
    let path = inbound_media_path(root, "writing", 42, "jpg");
    assert_eq!(
        path,
        std::path::PathBuf::from("/srv/.team/state/inbound-media/writing/42.jpg")
    );
}

#[test]
fn media_success_payload_includes_path_mime_size_and_omits_empty_caption() {
    let path = std::path::Path::new("/srv/.team/state/inbound-media/p/7.jpg");
    let s = media_success_payload(path, "", "image/jpeg", 1024);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["path"], "/srv/.team/state/inbound-media/p/7.jpg");
    assert_eq!(v["mime"], "image/jpeg");
    assert_eq!(v["size_bytes"], 1024);
    assert!(
        v.get("caption").is_none(),
        "empty caption omitted from payload"
    );
}

#[test]
fn media_success_payload_includes_caption_when_present() {
    let path = std::path::Path::new("/x.png");
    let s = media_success_payload(path, "look at this", "image/png", 32);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["caption"], "look at this");
}

#[test]
fn media_error_payload_carries_verbose_error_and_optional_caption() {
    // R12: media_error rows must surface the verbatim cause so the
    // agent can ack to the user with a real diagnostic.
    let s = media_error_payload("", "get_file: timed out");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["error"], "get_file: timed out");
    assert!(v.get("caption").is_none());

    let s = media_error_payload("a screenshot", "create file: permission denied");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["error"], "create file: permission denied");
    assert_eq!(v["caption"], "a screenshot");
}

#[test]
fn placeholder_then_success_update_round_trip() {
    // Pin the two-phase SQL pattern: insert a `media_pending` row,
    // then UPDATE to `image` with the final payload. The kind +
    // structured_payload must reflect the final state and the row
    // id must remain stable so the on-disk filename matches.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, kind, structured_payload)
         VALUES ('p', 'user:telegram', 'p:eng_lead', 'cap',
                 strftime('%s','now'), 'media_pending', '{}')",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();

    // Simulated post-download UPDATE.
    let payload =
        media_success_payload(std::path::Path::new("/x/p/3.jpg"), "cap", "image/jpeg", 128);
    conn.execute(
        "UPDATE messages SET kind = ?1, structured_payload = ?2 WHERE id = ?3",
        params!["image", payload, id],
    )
    .unwrap();

    let (kind, sp): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("image"));
    let v: serde_json::Value = serde_json::from_str(sp.as_deref().unwrap()).unwrap();
    assert_eq!(v["mime"], "image/jpeg");
}

#[test]
fn placeholder_then_error_update_writes_media_error_kind() {
    // Mirror of the success path for the failure mode (R12). After
    // the UPDATE the row's kind is `media_error` and the payload
    // carries the verbatim cause — operator's reply prompt has the
    // diagnostic in hand.
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, kind, structured_payload)
         VALUES ('p', 'user:telegram', 'p:eng_lead', '',
                 strftime('%s','now'), 'media_pending', '{}')",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();

    let payload = media_error_payload("", "download_file: 502 bad gateway");
    conn.execute(
        "UPDATE messages SET kind = 'media_error', structured_payload = ?1 WHERE id = ?2",
        params![payload, id],
    )
    .unwrap();

    let (kind, sp): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT kind, structured_payload FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("media_error"));
    assert!(sp.unwrap().contains("502 bad gateway"));
}

// ── #320 privileged-kind UPDATE guard ──────────────────────

/// Seed a `media_pending` placeholder row (the two-phase pattern the
/// bot uses before a download resolves) and return its rowid. Mirrors
/// the INSERT in `placeholder_then_success_update_round_trip` so the
/// guard tests exercise the exact shape `update_message_kind` runs on.
fn seed_media_pending(conn: &Connection) -> i64 {
    seed(conn);
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, kind, structured_payload)
         VALUES ('p', 'user:telegram', 'p:eng_lead', 'cap',
                 strftime('%s','now'), 'media_pending', '{}')",
        [],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn current_kind(conn: &Connection, id: i64) -> Option<String> {
    conn.query_row(
        "SELECT kind FROM messages WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn update_message_kind_refuses_system_and_leaves_row_untouched() {
    // The #320 tripwire: the privileged kind is insert-time only, so an
    // UPDATE that would set kind='system' must be refused. Crucially the
    // refusal is a true no-op — the row stays `media_pending`, never
    // half-mutated — because the guard bails *before* the UPDATE runs.
    // If a future change lets the UPDATE through, this test fails, which
    // is exactly the regression we want surfaced.
    let conn = Connection::open_in_memory().unwrap();
    let id = seed_media_pending(&conn);

    let result = update_message_kind(&conn, id, "system", "{}");

    assert!(result.is_err(), "setting privileged kind must be refused");
    assert_eq!(
        current_kind(&conn, id).as_deref(),
        Some("media_pending"),
        "refused UPDATE must not mutate the row"
    );
}

#[test]
fn update_message_kind_writes_legitimate_media_kinds() {
    // Happy path / no-behavior-change: the guard waves through every
    // kind a download legitimately resolves to (image / file / the
    // media_error fallback), updating both `kind` and the structured
    // payload. Each runs against its own placeholder row so the
    // assertions don't leak across cases.
    let success = media_success_payload(
        std::path::Path::new("/srv/.team/state/inbound-media/p/3.jpg"),
        "cap",
        "image/jpeg",
        128,
    );
    let error = media_error_payload("cap", "download_file: 502 bad gateway");
    let cases: [(&str, &str, &str); 3] = [
        ("image", success.as_str(), "image/jpeg"),
        ("file", success.as_str(), "image/jpeg"),
        ("media_error", error.as_str(), "502 bad gateway"),
    ];

    for (kind, payload, marker) in cases {
        let conn = Connection::open_in_memory().unwrap();
        let id = seed_media_pending(&conn);

        let changed = update_message_kind(&conn, id, kind, payload)
            .unwrap_or_else(|e| panic!("kind {kind:?} should be allowed: {e}"));
        assert_eq!(changed, 1, "kind {kind:?} should update exactly one row");

        let (stored_kind, sp): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, structured_payload FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_kind.as_deref(), Some(kind));
        assert!(
            sp.as_deref().unwrap().contains(marker),
            "kind {kind:?} payload should carry {marker:?}, got {sp:?}"
        );
    }
}

// ── T-101 voice STT mapping ────────────────────────────────
