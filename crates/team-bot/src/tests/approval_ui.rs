#[test]
fn approval_outcome_line_uses_approver_first_name() {
    assert_eq!(approval_outcome_line(true, "Hamed"), "✅ Approved by Hamed",);
    assert_eq!(
        approval_outcome_line(false, "Hamed"),
        "❌ Rejected by Hamed",
    );
}

#[test]
fn approval_outcome_line_handles_unicode_first_name() {
    assert_eq!(
        approval_outcome_line(true, "علیرضا"),
        "✅ Approved by علیرضا",
    );
}

// ── #299 multi-option decision helpers ───────────────────────

#[test]
fn decision_and_cancel_outcome_lines_name_the_chooser() {
    assert_eq!(
        decision_outcome_line("Ship it", "Hamed"),
        "✅ Ship it — chosen by Hamed",
    );
    assert_eq!(cancel_outcome_line("Hamed"), "🚫 Cancelled by Hamed");
    // Unicode chooser parity with the binary outcome-line test.
    assert_eq!(
        decision_outcome_line("گزینه", "علیرضا"),
        "✅ گزینه — chosen by علیرضا",
    );
}

#[test]
fn parse_callback_accepts_all_four_verbs() {
    assert_eq!(parse_callback("approve:7"), Some((7, CbAction::Approve)));
    assert_eq!(parse_callback("deny:7"), Some((7, CbAction::Deny)));
    assert_eq!(parse_callback("cancel:42"), Some((42, CbAction::Cancel)));
    assert_eq!(parse_callback("opt:42:2"), Some((42, CbAction::Opt(2))));
}

#[test]
fn parse_callback_rejects_malformed() {
    // No colon, unparseable id, unknown verb, bad opt idx, and
    // trailing junk all return None — handle_callback then ignores
    // the tap, preserving the prior unknown-data behavior.
    assert_eq!(parse_callback("approve"), None);
    assert_eq!(parse_callback("approve:x"), None);
    assert_eq!(parse_callback("frobnicate:7"), None);
    assert_eq!(parse_callback("opt:7:notanum"), None);
    assert_eq!(parse_callback("opt:7"), None);
    assert_eq!(parse_callback("approve:7:8"), None);
    assert_eq!(parse_callback("cancel:7:8"), None);
}

#[test]
fn decode_options_handles_null_and_garbage() {
    assert!(decode_options(None).is_empty());
    assert!(decode_options(Some("not json")).is_empty());
    // Entries missing label or value are filtered, not fatal.
    assert_eq!(
        decode_options(Some(r#"[{"label":"A","value":"a"},{"value":"b"}]"#)),
        vec![("A".to_string(), "a".to_string())],
    );
    assert_eq!(
        decode_options(Some(
            r#"[{"label":"Yes","value":"y"},{"label":"No","value":"n"}]"#
        )),
        vec![
            ("Yes".to_string(), "y".to_string()),
            ("No".to_string(), "n".to_string()),
        ],
    );
}

/// Pull `(text, callback_data)` out of a keyboard for assertions.
fn kb_pairs(kb: &InlineKeyboardMarkup) -> Vec<(String, String)> {
    use teloxide::types::InlineKeyboardButtonKind::CallbackData;
    kb.inline_keyboard
        .iter()
        .flatten()
        .map(|b| {
            let data = match &b.kind {
                CallbackData(d) => d.clone(),
                _ => panic!("expected callback button"),
            };
            (b.text.clone(), data)
        })
        .collect()
}

#[test]
fn approval_keyboard_empty_options_is_binary_back_compat() {
    // The binary card must stay byte-identical to the pre-#299
    // render: exactly Approve/Deny with `approve:{id}`/`deny:{id}`.
    let kb = approval_keyboard(7, &[]);
    assert_eq!(
        kb_pairs(&kb),
        vec![
            ("Approve".to_string(), "approve:7".to_string()),
            ("Deny".to_string(), "deny:7".to_string()),
        ],
    );
    assert_eq!(kb.inline_keyboard.len(), 1, "binary is a single row");
}

#[test]
fn approval_keyboard_multi_renders_options_then_cancel() {
    let opts = vec![
        ("Ship".to_string(), "ship".to_string()),
        ("Hold".to_string(), "hold".to_string()),
        ("Rework".to_string(), "rework".to_string()),
    ];
    let kb = approval_keyboard(9, &opts);
    assert_eq!(
        kb_pairs(&kb),
        vec![
            ("Ship".to_string(), "opt:9:0".to_string()),
            ("Hold".to_string(), "opt:9:1".to_string()),
            ("Rework".to_string(), "opt:9:2".to_string()),
            ("Cancel".to_string(), "cancel:9".to_string()),
        ],
    );
    assert_eq!(
        kb.inline_keyboard.len(),
        4,
        "one row per option + a Cancel row"
    );
}
