use rusqlite::params;
use teloxide::types::{ChatAction, ReplyParameters};
use teloxide::{net::Download, prelude::*};

use crate::{
    download::{media_size_pre_reject, BoundedWriter},
    State, SttRuntime, VOICE_INBOX_PREFIX,
};

/// Outcome of a transcription attempt. Three branches stay distinct in
/// code per the issue: a clean transcript, a "nothing recognizable"
/// signal (silence / music / noise — agent must not be disturbed), and
/// a hard failure (network, auth, provider down — surface verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SttOutcome {
    Ok(String),
    Skipped,
    Failed(String),
}

/// What the voice handler does next, derived purely from an `SttOutcome`.
/// Pulled out as data so the mapping is unit-testable without a real Bot.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct VoiceDecision {
    /// The Telegram reply (sent threaded under the voice message).
    pub(super) user_reply: String,
    /// The mailbox row text. `None` means no inbox row — used for both
    /// `Skipped` (don't disturb the agent) and `Failed` (no garbage
    /// transcript reaching the model).
    pub(super) inbox_text: Option<String>,
}

pub(super) fn map_voice_outcome(outcome: &SttOutcome) -> VoiceDecision {
    match outcome {
        SttOutcome::Ok(transcript) => VoiceDecision {
            user_reply: format!("🎙 \"{transcript}\""),
            inbox_text: Some(format!("{VOICE_INBOX_PREFIX} {transcript}")),
        },
        SttOutcome::Skipped => VoiceDecision {
            user_reply: "🎙 (couldn't capture anything. did you say something? — skipping)"
                .to_string(),
            inbox_text: None,
        },
        SttOutcome::Failed(err) => VoiceDecision {
            user_reply: format!("🎙 transcription failed: {err}"),
            inbox_text: None,
        },
    }
}

/// Inbound voice handler. Caller has already verified `msg.voice()` is
/// `Some`, the bot is manager-scoped, and an `SttRuntime` is configured.
/// Mirrors `handle_inbound_media`'s shape: download → provider call →
/// branch on outcome → optionally insert mailbox row → reply (threaded).
pub(super) async fn handle_voice(bot: &Bot, msg: &Message, state: &State) -> ResponseResult<()> {
    let manager = state.manager.as_deref().expect("checked by caller");
    let stt = state.stt.as_ref().expect("checked by caller");
    let Some((project, _)) = manager.split_once(':') else {
        return Ok(());
    };
    let Some(voice) = msg.voice() else {
        return Ok(());
    };
    let file_id = voice.file.id.to_string();
    let inbound_msg_id: i64 = msg.id.0 as i64;
    let reply_to = ReplyParameters::new(msg.id);

    // Soft cue while the provider call is in flight. Best-effort — a
    // typing-action failure should not cancel the actual transcription.
    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;

    let audio = match download_voice_bytes(bot, &file_id).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let decision = map_voice_outcome(&SttOutcome::Failed(err));
            bot.send_message(msg.chat.id, decision.user_reply)
                .reply_parameters(reply_to)
                .await?;
            return Ok(());
        }
    };

    let outcome = transcribe(&audio, stt).await;
    let decision = map_voice_outcome(&outcome);

    let mut insert_ok = false;
    if let Some(inbox_text) = decision.inbox_text.as_deref() {
        let c = state.conn.lock().await;
        // The verify-reply is about to tell the operator what was heard,
        // which raises their expectation that the agent got it. If the
        // INSERT fails we still send the reply (matches the existing
        // text/dm paths) but log loudly so the drop is diagnosable —
        // mirrors the `tracing::error!` in `handle_inbound_media`.
        match c.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, telegram_msg_id)
             VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'), ?4)",
            params![project, manager, inbox_text, inbound_msg_id],
        ) {
            Ok(_) => insert_ok = true,
            Err(e) => tracing::error!(
                "voice transcript INSERT failed for {manager}: {e} (operator was \
                 told what was heard but the agent will not receive it)"
            ),
        }
    }

    // #263: confirm the routing to the manager with the same `→ {target}` echo
    // string the text / dm / media paths use, as a plain send (no
    // `reply_parameters`) and before the transcript echo so the chat reads as
    // routing confirmation then what was heard. Note this echo is gated
    // STRICTER than those sibling paths on purpose: they echo unconditionally
    // (`let _ = c.execute(...)`), confirming the route even when the INSERT was
    // dropped, whereas `voice_should_echo_routing` requires the row to have
    // landed. Confirming a route that never reached the mailbox is exactly the
    // false "agent got it" this ticket exists to kill, so do not "align" this
    // gate away. (The siblings' unconditional echo is a separate latent gap.)
    if voice_should_echo_routing(&decision, insert_ok) {
        bot.send_message(msg.chat.id, format!("→ {manager}"))
            .await?;
    }

    bot.send_message(msg.chat.id, decision.user_reply)
        .reply_parameters(reply_to)
        .await?;
    Ok(())
}

/// #263: the operator gets the `→ {manager}` routing echo only when the
/// transcript actually reached the mailbox — a transcript was produced
/// (`inbox_text` is `Some`) AND its INSERT succeeded. A transcription failure
/// or skip (no `inbox_text`) and a dropped INSERT both yield no echo, so the
/// operator is never told an agent received audio it did not. Pure so the
/// three branches are unit-testable without a live `Bot`.
pub(super) fn voice_should_echo_routing(decision: &VoiceDecision, insert_ok: bool) -> bool {
    decision.inbox_text.is_some() && insert_ok
}

/// T-236: body of the operator-facing reply when voice arrives on a
/// manager-scoped bot that has no `speech_to_text` runtime configured.
/// Pure function so the unit test in this file can assert the
/// done-when content (voice-glyph, cause-named, two-fix-paths, docs
/// pointer) without spinning up a Telegram mock. Plain text matches
/// the existing voice-reply parse-mode (`handle_voice` sends without
/// `.parse_mode()`); backticks render literally and operators are
/// technical enough to parse them as `code quotes`.
pub(super) fn voice_stt_missing_reply() -> &'static str {
    "🎙 Voice isn't configured for this agent yet.\n\n\
     To enable, either run `/teamctl:adjust` in your project's Claude Code \
     to configure it conversationally, or add `interfaces.telegram.speech_to_text` \
     to the agent's project YAML manually.\n\n\
     Docs: https://teamctl.run/guides/telegram-bot/#voice-messages-optional"
}

/// T-236: inbound voice handler used when STT isn't configured. Caller
/// has already verified `msg.voice()` is `Some`, the bot is manager-
/// scoped, AND `state.stt.is_none()`. Mirrors `handle_voice`'s reply
/// pattern (threaded under the operator's voice message) so the
/// operator sees the response nested where they spoke.
pub(super) async fn handle_voice_stt_missing(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let reply_to = ReplyParameters::new(msg.id);
    bot.send_message(msg.chat.id, voice_stt_missing_reply())
        .reply_parameters(reply_to)
        .await?;
    Ok(())
}

/// Hard upper bound on voice-file bytes accepted from Telegram.
///
/// #279: Telegram caps voice notes at 60s OGG OPUS, which lands in the
/// tens of KB to a few hundred KB in practice (≤ ~500 KB at high
/// bitrate). 2 MiB gives ~4× headroom over real-world voice notes while
/// keeping a decisive DoS bound — a malicious or buggy upstream can't
/// make the bot pre-allocate gigabytes by reporting a huge `file.size`,
/// and the streaming download is bounded too (via [`BoundedWriter`]) so
/// a lying-small upstream can't flood the `Vec` mid-download either.
pub(super) const MAX_VOICE_BYTES: usize = 2 * 1024 * 1024;

// #279 voice-path pre-check helper — thin wrapper over the generic
// pair above so the voice pre-reject call site stays readable. No
// `voice_size_mid_reject` wrapper needed: `BoundedWriter::poll_write`
// calls `media_size_mid_reject` directly with the kind from the
// wrapper's construction.
pub(super) fn voice_size_pre_reject(reported: u32, max: usize) -> String {
    media_size_pre_reject("voice file", reported, max)
}

/// Stream a Telegram-hosted voice file into memory. Audio is ~tens of
/// KB per voice note (Telegram caps voice at 60s OGG OPUS), so we
/// collect into a `Vec<u8>` rather than touching disk — the bytes hit
/// the Groq multipart body and are dropped, no on-disk artifact
/// survives.
///
/// #279: both the pre-allocation and the actual download are bounded
/// by [`MAX_VOICE_BYTES`]. The pre-check on `file.size` fast-fails when
/// Telegram already says it's too large; the [`BoundedWriter`] wrapping
/// the destination `Vec<u8>` catches a lying-small upstream that tries
/// to stream past the ceiling mid-download. Either rejection bubbles
/// up as a clear, operator-visible error naming both numbers.
pub(super) async fn download_voice_bytes(bot: &Bot, file_id: &str) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    let file = bot
        .get_file(file_id.to_owned().into())
        .await
        .map_err(|e| format!("get_file: {e}"))?;

    if (file.size as usize) > MAX_VOICE_BYTES {
        return Err(voice_size_pre_reject(file.size, MAX_VOICE_BYTES));
    }

    let buf: Vec<u8> = Vec::with_capacity(file.size as usize);
    let mut bounded = BoundedWriter::new("voice file", buf, MAX_VOICE_BYTES);
    bot.download_file(&file.path, &mut bounded)
        .await
        .map_err(|e| format!("download_file: {e}"))?;

    let mut buf = bounded.into_inner();
    buf.flush().await.ok();
    Ok(buf)
}

/// Provider dispatch. v1 has one arm (`groq`); adding OpenAI Whisper or
/// whisper.cpp later is one match arm here, no plugin framework needed.
pub(super) async fn transcribe(audio: &[u8], stt: &SttRuntime) -> SttOutcome {
    match stt.provider.as_str() {
        "groq" => transcribe_groq(audio, stt).await,
        other => SttOutcome::Failed(format!("unknown stt provider `{other}`")),
    }
}

/// Groq Whisper transcription. The OpenAI-compatible endpoint accepts a
/// multipart form with `file`, `model`, optional `language`, and
/// `response_format=text` for a plain-text body. An empty/whitespace
/// response is treated as `Skipped` (silence, music, noise) — non-empty
/// transcripts are `Ok`.
pub(super) async fn transcribe_groq(audio: &[u8], stt: &SttRuntime) -> SttOutcome {
    let part = match reqwest::multipart::Part::bytes(audio.to_vec())
        .file_name("voice.ogg")
        .mime_str("audio/ogg")
    {
        Ok(p) => p,
        Err(e) => return SttOutcome::Failed(format!("multipart: {e}")),
    };
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", stt.model.clone())
        .text("response_format", "text");
    if let Some(lang) = &stt.language {
        form = form.text("language", lang.clone());
    }

    let resp = stt
        .http
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(&stt.api_key)
        .multipart(form)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return SttOutcome::Failed(format!("groq request: {e}")),
    };
    let status = resp.status();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => return SttOutcome::Failed(format!("groq read body: {e}")),
    };
    if !status.is_success() {
        return SttOutcome::Failed(format!("groq {status}: {}", body.trim()));
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        SttOutcome::Skipped
    } else {
        SttOutcome::Ok(trimmed.to_string())
    }
}
