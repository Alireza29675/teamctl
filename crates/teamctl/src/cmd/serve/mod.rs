//! Local browser workspace. Static assets are embedded; SQLite remains authoritative.
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Args as ClapArgs;
use rand::RngCore;
use serde_json::{json, Value};

mod api;
mod assets;
mod demo;
mod terminal;
#[cfg(test)]
mod tests;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Loopback bind address. Use an SSH tunnel for remote access.
    #[arg(long)]
    pub host: Option<IpAddr>,
    /// Port; 0 chooses an available port.
    #[arg(long)]
    pub port: Option<u16>,
    /// Do not launch the browser.
    #[arg(long)]
    pub no_open: bool,
    /// Environment variable containing a persistent access token (at least 32 characters).
    #[arg(long)]
    pub token_env: Option<String>,
    /// Override asset retention in days; 0 keeps assets indefinitely.
    #[arg(long)]
    pub cache_days: Option<u32>,
    /// Seed a disposable compose root with demonstration conversations; never starts providers.
    #[arg(long)]
    pub demo: bool,
}

pub(super) struct Workspace {
    root: PathBuf,
    db: PathBuf,
    assets: PathBuf,
    retention_days: Option<u32>,
    max_upload: usize,
    token: String,
    nonce: String,
    origin: String,
    authority: String,
    demo: bool,
    shells: terminal::Shells,
}

type Shared = Arc<Workspace>;
type ApiResult<T> = std::result::Result<T, ApiError>;

pub(super) struct ApiError(StatusCode, String);
impl ApiError {
    fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn missing(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("workspace request failed: {e:#}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Workspace request failed; see server log".into(),
        )
    }
}
impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}
impl From<team_core::chat::Error> for ApiError {
    fn from(e: team_core::chat::Error) -> Self {
        use team_core::chat::Error::*;
        match e {
            InvalidInput(s) => Self::bad(s),
            UnknownAgent(s) | NotFound(s) => Self::missing(s),
            Forbidden => Self(StatusCode::FORBIDDEN, "Interface access denied".into()),
            AmbiguousRoute | IdempotencyConflict | NoRoute => {
                Self(StatusCode::CONFLICT, e.to_string())
            }
            other => anyhow::Error::from(other).into(),
        }
    }
}

pub fn run(root: &Path, args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(start(root, args))
}

async fn start(root: &Path, args: Args) -> Result<()> {
    if args.demo {
        demo::prepare(root)?;
    }
    let compose = super::load(root)?;
    let report = team_core::validate::validate(&compose);
    if !report.is_empty() {
        bail!("invalid team definition: {:?}", report);
    }
    super::up::register_all_public(&compose)?;
    let db = compose.root.join(&compose.global.broker.path);
    let conn = team_core::chat::open(&db)?;
    team_core::interfaces::sync(&conn, &compose)?;
    let host = args.host.unwrap_or(
        compose
            .global
            .web
            .host
            .parse()
            .context("web.host must be an IP address")?,
    );
    if !host.is_loopback() {
        bail!("the workspace binds only to loopback; use an SSH tunnel for remote access");
    }
    let token = if let Some(name) = &args.token_env {
        let token =
            std::env::var(name).with_context(|| format!("missing access token in {name}"))?;
        if token.len() < 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(&b))
        {
            bail!("access token must contain at least 32 URL-safe characters");
        }
        token
    } else {
        random_id()
    };
    if compose.global.web.max_upload_bytes == 0
        || compose.global.web.max_upload_bytes > 1024 * 1024 * 1024
    {
        bail!("web.max_upload_bytes must be between 1 byte and 1 GiB");
    }
    if compose.global.web.cache_retention_days == Some(0) {
        bail!("web.cache_retention_days must be positive; use null to keep assets indefinitely");
    }
    let port = args.port.unwrap_or(compose.global.web.port);
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, port)).await?;
    let address = listener.local_addr()?;
    let browser_address = address;
    let origin = format!("http://{browser_address}");
    let retention = args
        .cache_days
        .map(|v| if v == 0 { None } else { Some(v) })
        .unwrap_or(compose.global.web.cache_retention_days);
    let state = Arc::new(Workspace {
        root: compose.root.clone(),
        db,
        assets: compose.root.join("state/web/assets"),
        retention_days: retention,
        max_upload: usize::try_from(compose.global.web.max_upload_bytes)
            .context("upload limit exceeds platform capacity")?,
        token,
        nonce: random_id(),
        authority: browser_address.to_string(),
        origin: origin.clone(),
        demo: args.demo,
        shells: Default::default(),
    });
    assets::ensure(&state, &conn)?;
    assets::cleanup(&state, &conn, now())?;
    if args.demo {
        demo::seed(&state, &conn)?;
    }
    drop(conn);
    let sweep = state.clone();
    let cleanup_task = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(3600));
        loop {
            timer.tick().await;
            let state = sweep.clone();
            let _ = blocking(move || {
                let c = team_core::chat::open(&state.db)?;
                assets::cleanup(&state, &c, now())?;
                Ok(())
            })
            .await;
        }
    });
    let url = format!("{origin}/#token={}", state.token);
    println!("teamctl workspace · {url}");
    println!(
        "Connected to {} defined agents · Ctrl+C stops this server",
        compose.agents().count()
    );
    if !args.no_open {
        open_browser(&url);
    }
    let result = axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    cleanup_task.abort();
    result?;
    Ok(())
}

fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/styles.css", get(styles))
        .route("/api/state", get(api::state))
        .route("/api/messages", get(api::messages).post(api::send))
        .route("/api/assets", post(assets::upload))
        .route("/api/assets/{id}", get(assets::download))
        .route(
            "/api/agents/{id}/terminal",
            get(terminal::read).post(terminal::write),
        )
        .route("/api/approvals/{id}", post(api::decide))
        .layer(DefaultBodyLimit::max(state.max_upload.max(256 * 1024)))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn guard(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if host != Some(state.authority.as_str()) {
        return ApiError(StatusCode::FORBIDDEN, "Invalid workspace host".into()).into_response();
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        if origin.to_str().ok() != Some(state.origin.as_str()) {
            return ApiError(
                StatusCode::FORBIDDEN,
                "Cross-origin workspace request denied".into(),
            )
            .into_response();
        }
    }
    if req
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        == Some("cross-site")
    {
        return ApiError(
            StatusCode::FORBIDDEN,
            "Cross-site workspace request denied".into(),
        )
        .into_response();
    }
    if req.uri().path().starts_with("/api/") {
        let supplied = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !supplied.is_some_and(|value| constant_eq(value.as_bytes(), state.token.as_bytes())) {
            return ApiError(
                StatusCode::UNAUTHORIZED,
                "Open the workspace using the access link printed by teamctl serve".into(),
            )
            .into_response();
        }
    }
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    let csp=format!("default-src 'none'; script-src 'self' 'nonce-{}'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; media-src 'self' blob: data:; connect-src 'self'; frame-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",state.nonce);
    if let Ok(csp) = HeaderValue::from_str(&csp) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    let mut d = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b) {
        d |= usize::from(x ^ y);
    }
    d == 0
}
fn random_id() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::from(anyhow::Error::from(e)))?
}
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(target_os = "windows")]
    let command = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = "xdg-open";
    if let Err(e) = std::process::Command::new(command).arg(url).spawn() {
        tracing::warn!("could not open browser: {e}; use the printed workspace link");
    }
}
async fn index(State(state): State<Shared>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../../assets/web/index.html").replace("__COMPONENT_NONCE__", &state.nonce),
    )
        .into_response()
}
async fn script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../../assets/web/app.js"),
    )
        .into_response()
}
async fn styles() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../../assets/web/styles.css"),
    )
        .into_response()
}
