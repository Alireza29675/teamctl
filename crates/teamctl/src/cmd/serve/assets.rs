use super::*;
use axum::{
    body::to_bytes,
    extract::{Path, State},
    http::HeaderMap,
};

pub(super) fn ensure(state: &Workspace, c: &rusqlite::Connection) -> Result<()> {
    team_core::web_assets::ensure(c, &state.root)
}
pub(super) fn cleanup(state: &Workspace, c: &rusqlite::Connection, time: f64) -> Result<usize> {
    team_core::web_assets::cleanup(c, &state.root, state.retention_days, time)
}
pub(super) fn descriptor(
    state: &Workspace,
    c: &rusqlite::Connection,
    id: &str,
) -> ApiResult<Value> {
    let descriptor = team_core::web_assets::descriptor(c, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::missing("Attachment not found"))?;
    if descriptor["expired"] == true {
        return Err(ApiError(
            StatusCode::GONE,
            "This attachment has expired".into(),
        ));
    }
    let path = state.assets.join(id);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| ApiError::missing("Attachment bytes are unavailable"))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(ApiError::missing("Attachment is unavailable"));
    }
    Ok(descriptor)
}

pub(super) async fn upload(
    State(state): State<Shared>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<Value>> {
    let name = headers
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("attachment")
        .to_owned();
    let name = decode_name(&name);
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = to_bytes(body, state.max_upload).await.map_err(|_| {
        ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Attachment exceeds upload limit".into(),
        )
    })?;
    if bytes.is_empty() {
        return Err(ApiError::bad("Attachment is empty"));
    }
    blocking(move || {
        let c = team_core::chat::open(&state.db)?;
        let asset = team_core::web_assets::cache_bytes(
            &c,
            &state.root,
            &bytes,
            &name,
            &mime,
            state.max_upload as u64,
        )
        .map_err(ApiError::from)?;
        Ok(Json(asset))
    })
    .await
}

pub(super) async fn download(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    blocking(move || {
        let c = team_core::chat::open(&state.db)?;
        let item = descriptor(&state, &c, &id)?;
        let path = state.assets.join(&id);
        // A bounded opened regular-file read also protects against a cache path swap.
        let cfg = team_core::compose::Attachments {
            max_size_bytes: state.max_upload as u64,
            allowed_roots: vec![state.assets.display().to_string()],
            ..Default::default()
        };
        let bytes = team_core::attachments::check_and_read(&cfg, &path, None)
            .map_err(|_| ApiError::missing("Attachment is unavailable"))?;
        let mime = item["mime_type"]
            .as_str()
            .unwrap_or("application/octet-stream");
        let inline =
            mime.starts_with("image/") || mime.starts_with("audio/") || mime.starts_with("video/");
        let disposition = format!(
            "{}; filename=\"attachment\"; filename*=UTF-8\'\'{}",
            if inline { "inline" } else { "attachment" },
            encode_name(item["name"].as_str().unwrap_or("attachment"))
        );
        let mut response = Body::from(bytes).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime)
                .map_err(|_| ApiError::bad("Invalid attachment MIME type"))?,
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .map_err(|_| ApiError::bad("Invalid attachment name"))?,
        );
        Ok(response)
    })
    .await
}

fn decode_name(value: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| "attachment".into())
}
fn encode_name(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
