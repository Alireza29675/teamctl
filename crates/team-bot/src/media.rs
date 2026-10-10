use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use teloxide::{net::Download, prelude::*};

use crate::{
    download::{media_size_pre_reject, BoundedWriter, MAX_DOWNLOAD_BYTES},
    inbound::confirm_inbound,
    State,
};

/// Photo vs. document — used to label the mailbox row's `kind`. The disk
/// path naming is the same either way (`<row_id>.<ext>`), so this just
/// drives the discriminator the agent reads via `inbox_peek`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum MediaKind {
    Image,
    File,
}

/// Resolved enough information to download and record an inbound media
/// message. `file_id` feeds `bot.get_file`; `extension` + `mime` ride into
/// the structured payload so the agent can pick a vision-content shape on
/// its own runtime.
pub(super) struct MediaIntent {
    pub(super) file_id: String,
    pub(super) extension: String,
    pub(super) mime: String,
    pub(super) kind: MediaKind,
}

/// Pick the largest photo size or fall back to a document; returns `None`
/// when the message carries neither (caller should defer to the text path).
/// Pulled out of `handle_inbound_media` so the picking rule is unit-testable
/// without standing up a fake `Bot`.
pub(super) fn classify_media_intent(msg: &Message) -> Option<MediaIntent> {
    if let Some(photos) = msg.photo() {
        // Telegram delivers photos as a list of `PhotoSize` thumbnails — pick
        // the largest by pixel count so the agent gets the highest fidelity
        // available. Telegram's photo storage is always JPEG regardless of
        // upload format, so we hard-code the extension + mime.
        let largest = photos
            .iter()
            .max_by_key(|p| (p.width as u64).saturating_mul(p.height as u64))?;
        return Some(MediaIntent {
            file_id: largest.file.id.to_string(),
            extension: "jpg".into(),
            mime: "image/jpeg".into(),
            kind: MediaKind::Image,
        });
    }
    if let Some(doc) = msg.document() {
        let mime = doc
            .mime_type
            .as_ref()
            .map(|m| m.essence_str().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let extension = extension_for_document(doc.file_name.as_deref(), &mime);
        // Telegram users often upload PNG/GIF as document (which preserves
        // the original bytes vs. the jpeg recompression of `photo`); route
        // those as Image so the agent's vision plumbing still picks them up.
        let kind = if mime.starts_with("image/") {
            MediaKind::Image
        } else {
            MediaKind::File
        };
        return Some(MediaIntent {
            file_id: doc.file.id.to_string(),
            extension,
            mime,
            kind,
        });
    }
    None
}

/// Pick the file extension to use for a document upload. Prefer the
/// uploaded filename's extension when it's a clean ASCII alphanumeric
/// suffix; fall back to the mime-type lookup table otherwise. Defensive
/// against names like `report.pdf.bak` (uses `bak`) or `evil/../traversal`
/// (the rsplit_once on '.' won't match a path separator on its own, but the
/// alphanumeric guard keeps the result tame).
pub(super) fn extension_for_document(filename: Option<&str>, mime: &str) -> String {
    if let Some(name) = filename {
        if let Some((_, ext)) = name.rsplit_once('.') {
            if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return ext.to_ascii_lowercase();
            }
        }
    }
    extension_from_mime(mime).into()
}

/// Mime → file-extension lookup. Covers the common Telegram-deliverable
/// shapes; everything unknown falls to `bin` so the on-disk file still
/// exists and an agent can re-mime it via libmagic if it cares.
pub(super) fn extension_from_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/zip" => "zip",
        "application/json" => "json",
        _ => "bin",
    }
}

/// Compose the on-disk path for an inbound media row. `media_root` is the
/// directory configured at startup (defaults to `<mailbox-parent>/inbound-
/// media/`); the per-project subdirectory keeps a multi-project mailbox
/// from colliding row ids across projects.
pub(super) fn inbound_media_path(
    media_root: &Path,
    project: &str,
    row_id: i64,
    extension: &str,
) -> PathBuf {
    media_root
        .join(project)
        .join(format!("{row_id}.{extension}"))
}

/// JSON shape for a successful media row. Empty captions are omitted so
/// agents reading the payload don't have to special-case the empty string.
pub(super) fn media_success_payload(
    path: &Path,
    caption: &str,
    mime: &str,
    size_bytes: u64,
) -> String {
    let mut payload = serde_json::json!({
        "path": path.display().to_string(),
        "mime": mime,
        "size_bytes": size_bytes,
    });
    if !caption.is_empty() {
        payload["caption"] = serde_json::Value::String(caption.to_string());
    }
    payload.to_string()
}

/// JSON shape for a failed media row (R12 — no silent drops). Captures the
/// verbatim error so the agent can ack to the user with a real diagnostic.
pub(super) fn media_error_payload(caption: &str, error: &str) -> String {
    let mut payload = serde_json::json!({ "error": error });
    if !caption.is_empty() {
        payload["caption"] = serde_json::Value::String(caption.to_string());
    }
    payload.to_string()
}

/// Stream a Telegram-hosted file to disk via `bot.get_file` + `bot.download_file`.
/// On any error returns a verbatim `String` so the caller can fold it into
/// the `media_error` mailbox row + the user-facing reply.
pub(super) async fn download_to(
    bot: &Bot,
    file_id: &str,
    path: &Path,
    dir: &Path,
) -> Result<u64, String> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("create_dir_all `{}`: {e}", dir.display()))?;
    let file = bot
        .get_file(file_id.to_owned().into())
        .await
        .map_err(|e| format!("get_file: {e}"))?;

    // #332: disk-fill counterpart of #279's RAM-OOM defense. Layer 1 —
    // fast-fail when Telegram already reports a size over the ceiling,
    // before creating any destination file on disk. Layer 2 (below) —
    // a `BoundedWriter` wrapping the file handle catches a lying-small
    // upstream that tries to stream past the ceiling mid-download.
    if (file.size as usize) > MAX_DOWNLOAD_BYTES {
        return Err(media_size_pre_reject(
            "media file",
            file.size,
            MAX_DOWNLOAD_BYTES,
        ));
    }

    let handle = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("create file `{}`: {e}", path.display()))?;
    let mut bounded = BoundedWriter::new("media file", handle, MAX_DOWNLOAD_BYTES);
    bot.download_file(&file.path, &mut bounded)
        .await
        .map_err(|e| format!("download_file: {e}"))?;

    let mut handle = bounded.into_inner();
    flush_download(&mut handle).await?;
    drop(handle);
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("metadata: {e}"))?;
    Ok(meta.len())
}

pub(super) async fn flush_download<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    writer
        .flush()
        .await
        .map_err(|error| format!("flush downloaded media: {error}"))
}

/// Branch from `handle_message` for inbound photo/document. The two-phase
/// SQL pattern reserves a hidden placeholder while downloading, then
/// atomically publishes completed media or a media_error with a fresh id.
pub(super) async fn handle_inbound_media(
    bot: &Bot,
    msg: &Message,
    state: &State,
) -> ResponseResult<()> {
    let Some(manager) = state.manager.as_deref() else {
        bot.send_message(
            msg.chat.id,
            "media uploads need a manager-scoped bot. \
             Run `teamctl bot up` to attach this bot to a manager.",
        )
        .await?;
        return Ok(());
    };
    let Some((project, _)) = manager.split_once(':') else {
        // `bot setup` validates this — defensive only.
        return Ok(());
    };
    let Some(intent) = classify_media_intent(msg) else {
        return Ok(());
    };
    let caption = msg.caption().unwrap_or("").to_string();

    // Reserve a stable file name while downloading. Pending rows are hidden
    // from inboxes; completion publishes a fresh id so a watcher that already
    // passed this placeholder still sees the finished message.
    let placeholder_payload = serde_json::json!({ "caption": caption }).to_string();
    let row_id_opt = {
        let c = state.conn.lock().await;
        match c.execute(
            "INSERT INTO messages
                (project_id, sender, recipient, text, sent_at, kind, structured_payload, telegram_msg_id)
             VALUES (?1, 'user:telegram', ?2, ?3, strftime('%s','now'),
                     'media_pending', ?4, ?5)",
            params![project, manager, &caption, placeholder_payload, i64::from(msg.id.0)],
        ) {
            Ok(_) => Some(c.last_insert_rowid()),
            Err(e) => {
                tracing::error!("inbound media: failed to insert placeholder row: {e}");
                None
            }
        }
    };
    let Some(row_id) = row_id_opt else {
        bot.send_message(
            msg.chat.id,
            "internal error: could not record the message; please retry.",
        )
        .await?;
        return Ok(());
    };

    let path = inbound_media_path(&state.media_root, project, row_id, &intent.extension);
    let dir = path.parent().unwrap_or(&state.media_root).to_path_buf();
    match download_to(bot, &intent.file_id, &path, &dir).await {
        Ok(size_bytes) => {
            let payload = media_success_payload(&path, &caption, &intent.mime, size_bytes);
            let kind = match intent.kind {
                MediaKind::Image => "image",
                MediaKind::File => "file",
            };
            let c = state.conn.lock().await;
            let finalized = finalize_media(&c, row_id, kind, &payload);
            drop(c);
            confirm_inbound(
                bot,
                msg.chat.id,
                finalized.map(|_| 1).map_err(|error| {
                    tracing::error!(%error, row_id, "inbound media finalization failed");
                    rusqlite::Error::InvalidQuery
                }),
                format!("→ {manager}"),
            )
            .await?;
        }
        Err(err) => {
            let payload = media_error_payload(&caption, &err);
            let c = state.conn.lock().await;
            if let Err(e) = finalize_media(&c, row_id, "media_error", &payload) {
                tracing::error!(
                    "inbound media: failed to record media_error on message {row_id}: {e}"
                );
            }
            drop(c);
            bot.send_message(msg.chat.id, format!("media download failed: {err}"))
                .await?;
        }
    }
    Ok(())
}

pub(super) fn finalize_media(
    conn: &Connection,
    staged_id: i64,
    kind: &str,
    payload: &str,
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let pending: bool = tx.query_row(
        "SELECT kind='media_pending' FROM messages WHERE id=?1",
        params![staged_id],
        |row| row.get(0),
    )?;
    if !pending {
        bail!("message {staged_id} is not pending media");
    }
    update_message_kind(&tx, staged_id, kind, payload)?;
    tx.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, thread_id, sent_at,
             telegram_msg_id, kind, structured_payload, delivery_mode)
         SELECT project_id, sender, recipient, text, thread_id, sent_at,
             telegram_msg_id, kind, structured_payload, delivery_mode
         FROM messages WHERE id=?1",
        params![staged_id],
    )?;
    let published_id = tx.last_insert_rowid();
    tx.execute("DELETE FROM messages WHERE id=?1", params![staged_id])?;
    tx.commit()?;
    Ok(published_id)
}

/// Update a message row's `kind` + structured payload, refusing any UPDATE that
/// would set the privileged `kind='system'` (#320).
///
/// `system` may only be *originated* by a `system:*` source at insert time (see
/// `is_privileged_kind` / `store::send_dm_kind`); no UPDATE has a legitimate
/// reason to mutate a row *to* it. Routing every `kind`-mutating UPDATE through
/// here guards the invariant on the UPDATE path, not insert-time only. Today's
/// callers only pass `image` / `file` / `media_error`; a future `system` is
/// refused (Err, no SQL run) rather than silently forging a lifecycle signal.
pub(super) fn update_message_kind(
    conn: &Connection,
    id: i64,
    kind: &str,
    payload: &str,
) -> Result<usize> {
    if team_core::mailbox::is_privileged_kind(kind) {
        bail!(
            "refusing UPDATE that would set privileged kind='{kind}' on message {id} \
             (#320): the privileged kind is insert-time only"
        );
    }
    conn.execute(
        "UPDATE messages SET kind = ?1, structured_payload = ?2 WHERE id = ?3",
        params![kind, payload, id],
    )
    .with_context(|| format!("update message {id} kind to '{kind}'"))
}
