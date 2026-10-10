use super::detector::*;
use super::*;

fn limit_patterns() -> Vec<CompiledPattern> {
    compile_patterns(&[RateLimitPattern {
        r#match: "LIMIT".into(),
        resets_at_capture: None,
        resets_in_capture: None,
    }])
    .unwrap()
}

#[test]
fn scanner_detects_split_and_unterminated_status_without_waiting_for_eof() {
    let patterns = limit_patterns();
    let mut scanner = OutputScanner::default();
    assert!(scanner.feed(b"before\nLI", &patterns).is_none());
    assert!(scanner.feed(b"MIT reached", &patterns).is_some());
    assert!(scanner.finish(&patterns).is_some());
}

#[test]
fn scanner_bounds_newline_free_output_and_keeps_matching_tail() {
    let patterns = limit_patterns();
    let mut scanner = OutputScanner::default();
    assert!(scanner.feed(&vec![b'x'; 1024 * 1024], &patterns).is_none());
    assert!(scanner.line.len() <= MAX_LINE_BYTES);
    assert!(scanner.feed(b" LIMIT", &patterns).is_some());
}

#[test]
fn reset_times_preserve_utc_offsets_ampm_and_rfc3339() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let expected = chrono::DateTime::parse_from_rfc3339("2026-10-10T16:30:00Z")
        .unwrap()
        .timestamp() as f64;
    for clock in [
        "16:30 UTC",
        "4:30 pm UTC",
        "18:30 +02:00",
        "2026-10-10T16:30:00Z",
    ] {
        assert_eq!(parse_clock_time_at(clock, now), Some(expected), "{clock}");
    }
    assert_eq!(parse_clock_time_at("16:30 XYZ", now), None);
    assert_eq!(
        parse_clock_time_at("4 pm UTC", now),
        Some(expected - 30.0 * 60.0)
    );
}

#[test]
fn embedded_claude_patterns_preserve_reset_timezone() {
    let runtimes = team_core::runtimes::embedded_defaults().unwrap();
    let patterns = compile_patterns(&runtimes["claude-code"].rate_limit_patterns).unwrap();
    for (line, clock) in [
        ("5-hour limit reached ∙ resets at 4pm UTC", "4pm UTC"),
        (
            "Claude usage limit reached. Your limit will reset at 18:00 +02:00.",
            "18:00 +02:00",
        ),
    ] {
        let event = scan_line(line.as_bytes(), &patterns).unwrap();
        assert_eq!(event.resets_at, parse_clock_time(clock), "{line}");
    }
    assert!(
        scan_line(b"5-hour limit reached resets at 4pm XYZ", &patterns)
            .unwrap()
            .resets_at
            .is_none()
    );
}

#[test]
fn unparseable_absolute_reset_falls_back_to_relative_capture() {
    let patterns = compile_patterns(&[RateLimitPattern {
        r#match: "LIMIT".into(),
        resets_at_capture: Some("at (\\S+)".into()),
        resets_in_capture: Some("in (\\d+s)".into()),
    }])
    .unwrap();
    let before = now();
    let event = scan_line(b"LIMIT at nonsense retry in 30s", &patterns).unwrap();
    assert!((event.resets_at.unwrap() - before - 30.0).abs() < 1.0);
}

#[test]
fn rate_limit_observation_is_durable_before_hooks_or_process_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailbox.db");
    let event = RlEvent {
        raw: "LIMIT".into(),
        resets_at: None,
    };
    let (row, _) = record_hit(&path, "p:a", "fake", &event).unwrap();
    let conn = Connection::open(path).unwrap();
    let handled: Option<f64> = conn
        .query_row(
            "SELECT handled_at FROM rate_limits WHERE id=?1",
            [row],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(handled, None);
}

#[test]
fn split_limit_prefix_updates_same_durable_observation_with_reset_capture() {
    let patterns = compile_patterns(&[RateLimitPattern {
        r#match: "Claude usage limit reached".into(),
        resets_at_capture: Some("reset at (\\d{2}:\\d{2} UTC)".into()),
        resets_in_capture: None,
    }])
    .unwrap();
    let mut scanner = OutputScanner::default();
    let first = scanner
        .feed(b"Claude usage limit reached.", &patterns)
        .unwrap();
    assert!(first.resets_at.is_none());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailbox.db");
    let (row, _) = record_hit(&path, "p:a", "claude-code", &first).unwrap();
    let final_event = scanner
        .feed(
            b" Your limit will reset at 16:00 UTC.\nnext prompt",
            &patterns,
        )
        .unwrap();
    assert!(final_event.resets_at.is_some());
    assert_eq!(scanner.line, b"next prompt");
    update_hit(&path, row, &final_event).unwrap();
    let conn = Connection::open(path).unwrap();
    let (count, reset, raw): (usize, Option<f64>, String) = conn
        .query_row(
            "SELECT count(*), resets_at, raw_match FROM rate_limits",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(reset, final_event.resets_at);
    assert!(raw.ends_with("16:00 UTC."));
}

#[test]
fn duration_overflow_is_rejected() {
    assert_eq!(parse_duration("18446744073709551615h"), None);
    assert_eq!(parse_duration("18446744073709551615s 1s"), None);
}

#[test]
fn duration_parses_compound() {
    assert_eq!(parse_duration("5h 15m"), Some(5 * 3600 + 15 * 60));
    assert_eq!(parse_duration("30m"), Some(30 * 60));
    assert_eq!(parse_duration("120s"), Some(120));
    assert_eq!(parse_duration("2 hours"), Some(2 * 3600));
    assert_eq!(parse_duration(""), None);
}

#[test]
fn compile_patterns_works() {
    let v = vec![RateLimitPattern {
        r#match: "(?i)limit reached".into(),
        resets_at_capture: Some("(?i)at ([0-9]+(?:am|pm))".into()),
        resets_in_capture: None,
    }];
    let c = compile_patterns(&v).unwrap();
    assert_eq!(c.len(), 1);
    assert!(c[0].matcher.is_match("Limit reached, please wait"));
}

#[test]
fn strip_ansi_csi() {
    let input = b"\x1b[31mhello\x1b[0m world";
    assert_eq!(strip_ansi(input), b"hello world");
}

#[test]
fn strip_ansi_osc() {
    let input = b"\x1b]0;title\x07after";
    assert_eq!(strip_ansi(input), b"after");
}

#[cfg(unix)]
#[test]
fn winch_handler_flips_pending_flag() {
    // Installing the handler is idempotent — we can call it from a
    // test without disturbing the rest of the process. Raising
    // SIGWINCH on ourselves is synchronous: by the time `raise`
    // returns, the handler has run.
    install_winch_handler();
    WINCH_PENDING.store(false, Ordering::SeqCst);
    unsafe {
        libc::raise(libc::SIGWINCH);
    }
    assert!(
        WINCH_PENDING.swap(false, Ordering::SeqCst),
        "SIGWINCH should have flipped WINCH_PENDING via the handler"
    );
}

#[test]
fn scan_line_matches_with_ansi_codes() {
    let patterns = compile_patterns(&[RateLimitPattern {
        r#match: "(?i)limit reached".into(),
        resets_at_capture: None,
        resets_in_capture: None,
    }])
    .unwrap();
    let line = b"\x1b[33mLimit reached!\x1b[0m";
    assert!(scan_line(line, &patterns).is_some());
}
