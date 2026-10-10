#[test]
fn classify_kind_routes_reaction() {
    // T-086-E adds a fourth kind alongside text/image/file. Future
    // refactors that drop this arm should fail this test rather
    // than silently degrading reactions to UnknownFallback (which
    // would surface as text noise rather than an actual reaction).
    assert_eq!(classify_kind(Some("reaction")), DispatchKind::Reaction);
}

#[test]
fn classify_kind_unknown_fallback_unchanged_for_other_strings() {
    // Regression guard: T-086-E must not accidentally turn the
    // `UnknownFallback` arm into a reaction match — only the
    // exact string "reaction" routes to the new arm.
    assert_eq!(
        classify_kind(Some("reactions")),
        DispatchKind::UnknownFallback
    );
    assert_eq!(classify_kind(Some("react")), DispatchKind::UnknownFallback);
}

#[test]
fn parse_reaction_payload_extracts_telegram_msg_id_and_emoji() {
    let p =
        parse_reaction_payload(r#"{"telegram_msg_id":4242,"emoji":"👀"}"#).expect("payload parses");
    assert_eq!(p.telegram_msg_id, 4242);
    assert_eq!(p.emoji, "👀");
}

#[test]
fn parse_reaction_payload_returns_none_on_missing_fields() {
    assert!(parse_reaction_payload("not json").is_none());
    assert!(
        parse_reaction_payload(r#"{"emoji":"👍"}"#).is_none(),
        "missing telegram_msg_id"
    );
    assert!(
        parse_reaction_payload(r#"{"telegram_msg_id":7}"#).is_none(),
        "missing emoji"
    );
}

#[test]
fn parse_reaction_payload_returns_none_on_wrong_types() {
    // Defence-in-depth: a malformed payload (string in place of
    // i64) shouldn't crash, just return None so the dispatcher
    // falls back to the log-and-skip arm.
    assert!(
        parse_reaction_payload(r#"{"telegram_msg_id":"oops","emoji":"👍"}"#).is_none(),
        "string telegram_msg_id"
    );
    assert!(
        parse_reaction_payload(r#"{"telegram_msg_id":7,"emoji":42}"#).is_none(),
        "non-string emoji"
    );
}

// ── T-102 typing-indicator helpers ───────────────────────────

#[test]
fn classify_kind_routes_typing() {
    // Sibling to the reaction dispatch test. A future refactor that
    // drops the `Some("typing")` arm degrades the indicator to
    // `UnknownFallback`, which would post the row's empty `text` to
    // the chat — we want that to fail this test rather than ship.
    assert_eq!(classify_kind(Some("typing")), DispatchKind::Typing);
}

#[test]
fn classify_kind_unknown_fallback_unchanged_for_typing_lookalikes() {
    // Regression guard mirroring the reaction test: only the exact
    // string "typing" routes; "type" / "typings" stay in the
    // unknown bucket so a typo on the MCP side is loud, not silent.
    assert_eq!(classify_kind(Some("type")), DispatchKind::UnknownFallback);
    assert_eq!(
        classify_kind(Some("typings")),
        DispatchKind::UnknownFallback
    );
}

#[test]
fn extend_typing_window_inserts_new_entry_with_deadline() {
    // First call for a chat: map gains an entry whose deadline is
    // exactly `now + ceiling`. The pure helper is the shared core
    // between the dispatcher and the refresh loop, so it has to
    // round-trip the math without rounding surprises.
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    let now = Instant::now();
    let ceiling = Duration::from_secs(10);
    let deadline = extend_typing_window(&mut map, ChatId(42), now, ceiling);
    assert_eq!(deadline, now + ceiling);
    assert_eq!(map.get(&ChatId(42)), Some(&(now + ceiling)));
}

#[test]
fn extend_typing_window_resets_existing_deadline_on_second_call() {
    // Spec's "second call extends the window (resets the 10s
    // clock)": the new deadline is computed from the *new* `now`,
    // not appended to the old one. A monotonic clock makes "newer
    // is later" the correct invariant.
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    let t0 = Instant::now();
    let ceiling = Duration::from_secs(10);
    extend_typing_window(&mut map, ChatId(7), t0, ceiling);
    let t1 = t0 + Duration::from_secs(3);
    let deadline = extend_typing_window(&mut map, ChatId(7), t1, ceiling);
    assert_eq!(deadline, t1 + ceiling);
    assert_eq!(map.get(&ChatId(7)), Some(&(t1 + ceiling)));
}

#[test]
fn clear_typing_window_removes_present_entry_and_reports_true() {
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    let now = Instant::now();
    extend_typing_window(&mut map, ChatId(1), now, Duration::from_secs(10));
    assert!(clear_typing_window(&mut map, ChatId(1)));
    assert!(!map.contains_key(&ChatId(1)));
}

#[test]
fn clear_typing_window_returns_false_when_chat_not_tracked() {
    // A text/image/file dispatch happens whether or not a typing
    // window was open; the helper has to stay no-op-safe in the
    // absent case rather than panicking.
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    assert!(!clear_typing_window(&mut map, ChatId(99)));
}

#[test]
fn refresh_typing_windows_drops_expired_and_returns_active() {
    // The refresh loop wakes every ~4s, drops anything past its
    // ceiling, and re-fires `sendChatAction` on whatever's left.
    // Pinning both halves of that: drop the expired entry, return
    // the still-active chats.
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    let now = Instant::now();
    map.insert(ChatId(1), now + Duration::from_secs(2));
    map.insert(ChatId(2), now - Duration::from_millis(10));
    let active = refresh_typing_windows(&mut map, now);
    assert_eq!(active, vec![ChatId(1)]);
    assert!(map.contains_key(&ChatId(1)));
    assert!(!map.contains_key(&ChatId(2)));
}

#[test]
fn refresh_typing_windows_returns_empty_on_empty_map() {
    // Steady state: no agent has called `show_typing` recently, so
    // the refresh loop's tick produces no Telegram traffic.
    let mut map: HashMap<ChatId, Instant> = HashMap::new();
    let active = refresh_typing_windows(&mut map, Instant::now());
    assert!(active.is_empty());
}

// ── T-086-B reply_parameters dispatch ────────────────────────

#[test]
fn reply_parameters_for_returns_none_when_telegram_msg_id_is_none() {
    // Back-compat pin: rows without a threading target produce no
    // ReplyParameters so the dispatcher doesn't attach them and the
    // message lands as a fresh post.
    assert!(reply_parameters_for(None).is_none());
}

#[test]
fn reply_parameters_for_returns_some_when_telegram_msg_id_is_set() {
    // Affirmative pin: present id → constructed `ReplyParameters`
    // ready for the teloxide builder. The actual MessageId carried
    // is asserted via the by-value PartialEq.
    let rp = reply_parameters_for(Some(12345)).expect("Some when set");
    assert_eq!(rp.message_id, MessageId(12345));
}

#[test]
fn reply_parameters_for_safely_casts_within_i32_range() {
    // Telegram message ids are i32; Rust API takes i64 for SQLite
    // ergonomics. Pin that values comfortably within i32 range
    // round-trip exactly — guards against a future refactor that
    // drops the `as i32` cast and ends up with a wrap-around bug
    // on large but valid ids.
    let id: i64 = 2_000_000_000;
    let rp = reply_parameters_for(Some(id)).expect("Some when set");
    assert_eq!(rp.message_id, MessageId(id as i32));
}

// ── T-086-C inbound media helpers ─────────────────────────────
