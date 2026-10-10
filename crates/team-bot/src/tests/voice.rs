#[test]
fn map_voice_outcome_ok_yields_quoted_reply_and_prefixed_inbox_row() {
    let d = map_voice_outcome(&SttOutcome::Ok("hello team".into()));
    assert_eq!(d.user_reply, "🎙 \"hello team\"");
    assert_eq!(
        d.inbox_text.as_deref(),
        Some("🎙 (transcribed voice, may have misspellings): hello team")
    );
    // Pin the prefix constant so future renames flag through this test.
    assert!(d
        .inbox_text
        .as_deref()
        .unwrap()
        .starts_with(VOICE_INBOX_PREFIX));
}

#[test]
fn map_voice_outcome_skipped_yields_no_inbox_row() {
    let d = map_voice_outcome(&SttOutcome::Skipped);
    assert!(d.inbox_text.is_none());
    // Skipped vs Failed must stay distinct (per issue, conflating
    // them is a UX bug). The skipped phrasing asks if the operator
    // said something — failed messages name the failure.
    assert!(d.user_reply.contains("couldn't capture anything"));
    assert!(!d.user_reply.contains("failed"));
}

#[test]
fn map_voice_outcome_failed_yields_no_inbox_row_and_surfaces_error() {
    let d = map_voice_outcome(&SttOutcome::Failed("network down".into()));
    assert!(d.inbox_text.is_none());
    assert!(d.user_reply.contains("failed"));
    assert!(d.user_reply.contains("network down"));
    // And the failure shape must NOT match the skipped phrasing —
    // the operator needs to know whether they were heard or not.
    assert!(!d.user_reply.contains("couldn't capture"));
}

#[test]
fn voice_echoes_routing_only_on_a_successful_insert() {
    // #263, the three acceptance branches. Success: a transcript was
    // produced and its INSERT landed → the operator gets the `→ {manager}`
    // routing echo.
    let ok = map_voice_outcome(&SttOutcome::Ok("hello team".into()));
    assert!(
        voice_should_echo_routing(&ok, true),
        "success path must confirm routing"
    );
    // Dropped INSERT: a transcript exists but the row failed to write →
    // no echo, so the operator is never told an agent received it.
    assert!(
        !voice_should_echo_routing(&ok, false),
        "a dropped INSERT must not claim routing"
    );
    // Transcription failure / skip: no transcript → no row, no echo,
    // regardless of the insert flag (the INSERT never runs in the handler).
    let failed = map_voice_outcome(&SttOutcome::Failed("network down".into()));
    assert!(
        !voice_should_echo_routing(&failed, true),
        "a transcription failure must not confirm routing"
    );
    let skipped = map_voice_outcome(&SttOutcome::Skipped);
    assert!(
        !voice_should_echo_routing(&skipped, true),
        "an empty/skipped capture must not confirm routing"
    );
}

#[test]
fn voice_inbox_prefix_matches_issue_spec() {
    // Pin the exact string the issue spec calls for, so a future
    // rename can't silently drift the model-facing contract.
    assert_eq!(
        VOICE_INBOX_PREFIX,
        "🎙 (transcribed voice, may have misspellings):"
    );
}

// ── T-236 voice-STT-missing config hint ────────────────────────

#[test]
fn voice_stt_missing_reply_carries_operator_actionable_hints() {
    // Done-when contract (issue #236): when voice arrives on a
    // manager-scoped bot without STT configured, the reply must
    // convey (a) confirmation the voice was received (operator's
    // primary confusion), (b) why nothing's happening, (c) two
    // clear paths to fix it — `/teamctl:adjust` (conversational)
    // AND manual project YAML — and (d) a docs pointer. Pin each
    // piece so wording can evolve without dropping a contract.
    let body = voice_stt_missing_reply();
    assert!(
        body.starts_with("🎙"),
        "reply must lead with the voice glyph so the operator sees \
         this is about their voice message: {body}"
    );
    assert!(
        body.contains("Voice isn't configured"),
        "reply must name the cause so the operator knows what to fix: {body}"
    );
    assert!(
        body.contains("/teamctl:adjust"),
        "reply must surface the conversational fix path: {body}"
    );
    assert!(
        body.contains("interfaces.telegram.speech_to_text"),
        "reply must surface the YAML key for the manual fix path: {body}"
    );
    assert!(
        body.contains("https://teamctl.run/"),
        "reply must include a docs pointer the operator can open: {body}"
    );
}

// ── #279: voice-download size ceiling ──────────────────────────

#[test]
fn voice_size_pre_reject_names_both_numbers() {
    // Operator-visible error must surface both the reported size
    // and the ceiling so the cause is obvious without log-diving.
    let msg = voice_size_pre_reject(5_000_000, MAX_VOICE_BYTES);
    assert!(msg.contains("5000000"), "msg names reported: {msg}");
    assert!(
        msg.contains(&MAX_VOICE_BYTES.to_string()),
        "msg names max: {msg}"
    );
    assert!(msg.contains("too large"), "msg flags the rejection: {msg}");
}

#[test]
fn voice_path_mid_reject_names_both_numbers_and_distinguishes() {
    // The voice-path BoundedWriter constructs its mid-reject via
    // `media_size_mid_reject("voice file", ...)` — verify that
    // call shape still produces a distinguishable message naming
    // both numbers (#279 invariant, kept by the #332 generalization
    // of the helper).
    let msg = media_size_mid_reject("voice file", 1_000_000, 1_500_000, MAX_VOICE_BYTES);
    assert!(msg.contains("voice file"), "msg carries voice kind: {msg}");
    assert!(msg.contains("2500000"), "msg names cumulative: {msg}");
    assert!(
        msg.contains(&MAX_VOICE_BYTES.to_string()),
        "msg names max: {msg}"
    );
    assert!(
        msg.contains("mid-download"),
        "msg distinguishes from pre-reject: {msg}"
    );
}

#[tokio::test]
async fn bounded_writer_passes_through_when_under_max() {
    use tokio::io::AsyncWriteExt;
    let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 16);
    bw.write_all(b"hello").await.unwrap();
    bw.write_all(b" world").await.unwrap();
    bw.flush().await.unwrap();
    assert_eq!(bw.into_inner(), b"hello world");
}

#[tokio::test]
async fn bounded_writer_allows_exactly_max() {
    // Boundary: cumulative `max` bytes succeed; only `max + 1`
    // trips the abort. Mirrors the strict `>` comparison.
    use tokio::io::AsyncWriteExt;
    let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 5);
    bw.write_all(b"hello").await.unwrap();
    assert_eq!(bw.into_inner(), b"hello");
}

#[tokio::test]
async fn bounded_writer_errors_when_chunk_would_exceed_max() {
    use tokio::io::AsyncWriteExt;
    let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 4);
    // First write fits exactly.
    bw.write_all(b"abcd").await.unwrap();
    // Second write would push past — must error, must not append.
    let err = bw.write_all(b"e").await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("mid-download"),
        "error must use media_size_mid_reject format: {err}"
    );
    assert_eq!(
        bw.into_inner().as_slice(),
        b"abcd",
        "rejected chunk must not append"
    );
}

#[tokio::test]
async fn bounded_writer_errors_on_single_oversize_chunk() {
    // A single write larger than `max` must also abort, with
    // nothing written — covers a lying upstream that dumps the
    // whole body in one chunk.
    use tokio::io::AsyncWriteExt;
    let mut bw = BoundedWriter::new("voice file", Vec::<u8>::new(), 4);
    let err = bw.write_all(b"abcde").await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("mid-download"));
    assert!(
        bw.into_inner().is_empty(),
        "no bytes must land when the chunk overshoots"
    );
}

// ── #332: disk-fill defense on the `download_to` path ──────────

#[test]
fn media_size_pre_reject_carries_kind_and_names_both_numbers() {
    // The `kind` label distinguishes voice (#279, RAM-OOM) from
    // media (#332, disk-fill) so operators can tell which boundary
    // fired without log-diving.
    let msg = media_size_pre_reject("media file", 60_000_000, MAX_DOWNLOAD_BYTES);
    assert!(msg.contains("media file"), "msg carries kind: {msg}");
    assert!(msg.contains("60000000"), "msg names reported: {msg}");
    assert!(
        msg.contains(&MAX_DOWNLOAD_BYTES.to_string()),
        "msg names max: {msg}"
    );
    assert!(msg.contains("too large"), "msg flags the rejection: {msg}");
}

#[test]
fn media_size_mid_reject_carries_kind_and_distinguishes() {
    let msg = media_size_mid_reject("media file", 40_000_000, 20_000_000, MAX_DOWNLOAD_BYTES);
    assert!(msg.contains("media file"), "msg carries kind: {msg}");
    assert!(msg.contains("60000000"), "msg names cumulative: {msg}");
    assert!(
        msg.contains(&MAX_DOWNLOAD_BYTES.to_string()),
        "msg names max: {msg}"
    );
    assert!(
        msg.contains("mid-download"),
        "msg distinguishes from pre-reject: {msg}"
    );
}

#[tokio::test]
async fn bounded_writer_aborts_mid_stream_on_tokio_fs_file_without_excess_disk_bytes() {
    // #332 disk-path pin: `BoundedWriter` wrapping a real
    // `tokio::fs::File` aborts the over-cap chunk WITHOUT having
    // written it to disk. Proves the streaming-abort defense holds
    // at the actual sink `download_to` uses, not just the `Vec<u8>`
    // sink from #279 — the `>` check fires before
    // `inner.poll_write`, so no disk write occurs for the rejected
    // chunk.
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("media");
    let f = tokio::fs::File::create(&path).await.unwrap();
    let mut bw = BoundedWriter::new("media file", f, 4);
    bw.write_all(b"abcd").await.unwrap();
    let err = bw.write_all(b"e").await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("media file"),
        "mid-reject must carry the `media file` kind: {err}"
    );
    let mut f = bw.into_inner();
    f.flush().await.ok();
    drop(f);
    let bytes = tokio::fs::read(&path).await.unwrap();
    assert_eq!(
        bytes, b"abcd",
        "rejected chunk must not have hit disk — bounded handle saw \
         the over-cap chunk and aborted before `inner.poll_write`"
    );
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn max_download_bytes_strictly_exceeds_max_voice_bytes() {
    // Bump-policy invariant: the disk path accepts strictly larger
    // files than the voice path (different threat model — disk
    // tolerates more than RAM-OOM). Catches an accidental swap or
    // misordering of the constants. The assertion is on consts by
    // design (that's the invariant); clippy's `assertions_on_
    // constants` would flag it otherwise.
    assert!(
        MAX_DOWNLOAD_BYTES > MAX_VOICE_BYTES,
        "MAX_DOWNLOAD_BYTES ({MAX_DOWNLOAD_BYTES}) must exceed \
         MAX_VOICE_BYTES ({MAX_VOICE_BYTES})"
    );
}
