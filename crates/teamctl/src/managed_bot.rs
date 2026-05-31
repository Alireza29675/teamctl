//! Telegram **Managed Bots** client — the manager-bot side of teamctl's
//! managed-bots flow (#132 PR-2).
//!
//! `teamctl bot setup` (the #344 wizard) uses one *manager bot* to spawn a
//! child bot per agent, then reads each child's token. The flow:
//!
//! 1. Emit the creation link [`ManagedBotClient::creation_link`] — the operator
//!    clicks it and confirms in Telegram.
//! 2. The manager bot receives a `managed_bot` update;
//!    [`ManagedBotClient::poll_for_managed_bot`] long-polls until it arrives,
//!    yielding a [`ManagedBotUpdated`] whose `bot.id` is the new child.
//! 3. [`ManagedBotClient::get_managed_bot_token`] fetches that child's token,
//!    which the wizard writes into the per-agent `bot_token_env` slot.
//!
//! This is a small raw-`reqwest` client: teloxide (the normal Telegram path)
//! does not expose the managed-bots methods. Cadence + backoff are hardcoded
//! consts (not config-tunable for v1 — YAGNI, per PR-1). The manager token is
//! carried in the URL path and is **never logged**.
//!
//! v1 surface (Telegram Bot API 9.6):
//! - `getManagedBotToken` — fetch a child's token by its `user_id`; the token
//!   comes back as a bare string.
//! - The `managed_bot` (`ManagedBotUpdated`) update + the `t.me/newbot` creation
//!   link — the spawn flow.
//!
//! Out of scope for v1 (also 9.6): `replaceManagedBotToken` (token rotation) and
//! the access-settings methods (`get`/`setManagedBotAccessSettings`) — the
//! wizard only needs token-fetch.

// Landed ahead of its consumer: the #344 `teamctl bot setup` wizard wires this
// client into the managed-bots setup flow (next PR in the #132 stack). Until
// that lands nothing in the binary calls it, so allow dead_code to keep the
// stacked sequence green; the tests below exercise the full surface.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// Production Telegram Bot API base. Overridable at test time via
/// `TEAMCTL_TG_API_BASE` so mock-HTTP tests can target a local server.
const DEFAULT_API_BASE: &str = "https://api.telegram.org";
const API_BASE_ENV: &str = "TEAMCTL_TG_API_BASE";

/// Poll cadence while waiting for the operator to confirm child-bot creation.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Exponential backoff bounds for transient poll/network errors.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Long-poll timeout (seconds) for `getUpdates`.
const LONG_POLL_SECS: u64 = 25;

/// Telegram response envelope: `{ ok, result, description }`.
#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    // Missing `Option` fields deserialize to `None` without `#[serde(default)]`
    // — and `default` here would wrongly demand `T: Default`.
    result: Option<T>,
    description: Option<String>,
}

/// Minimal Telegram `User` — bots are users; the managed-bots flow only needs
/// the id (other fields are ignored by serde). The new child bot's id arrives
/// as `ManagedBotUpdated.bot.id`.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
}

/// The `managed_bot` update payload (Bot API 9.6). The new child bot's id is
/// `bot.id` — pass it to [`ManagedBotClient::get_managed_bot_token`].
#[derive(Debug, Clone, Deserialize)]
pub struct ManagedBotUpdated {
    /// User that created the managed bot.
    pub user: User,
    /// The newly created managed bot (a bot user).
    pub bot: User,
}

/// One Telegram update, narrowed to the fields this client consumes.
#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    managed_bot: Option<ManagedBotUpdated>,
}

/// Client for the manager-bot side of the managed-bots flow.
pub struct ManagedBotClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl ManagedBotClient {
    /// Build from the manager bot's token (already resolved from env via
    /// [`token_from_env`]).
    pub fn new(manager_token: String) -> Self {
        let base = std::env::var(API_BASE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        Self {
            http: reqwest::Client::new(),
            base,
            token: manager_token,
        }
    }

    /// The link the operator clicks to spawn a child bot under the manager:
    /// `https://t.me/newbot/{manager}/{suggested}`.
    pub fn creation_link(manager_username: &str, suggested_username: &str) -> String {
        format!("https://t.me/newbot/{manager_username}/{suggested_username}")
    }

    /// POST a Bot API method and decode its `result`. The token is carried in
    /// the URL path; it is never logged (only the method name appears in errors).
    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}/bot{}/{}", self.base, self.token, method);
        let body =
            serde_json::to_vec(params).with_context(|| format!("serialize {method} request"))?;
        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .with_context(|| format!("{method} request failed"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("{method} read body"))?;
        let envelope: ApiResponse<T> =
            serde_json::from_str(&text).with_context(|| format!("{method} decode response"))?;
        if !status.is_success() || !envelope.ok {
            let desc = envelope.description.unwrap_or_else(|| text.clone());
            bail!("{method} failed ({status}): {desc}");
        }
        envelope
            .result
            .ok_or_else(|| anyhow!("{method}: ok response carried no result"))
    }

    /// `getManagedBotToken` — fetch a spawned child bot's token.
    ///
    /// `user_id` is the managed bot's user id (Telegram bots are users) — i.e.
    /// `ManagedBotUpdated.bot.id`. The API returns the token as a bare string
    /// (`{"ok":true,"result":"123:ABC"}`).
    pub async fn get_managed_bot_token(&self, user_id: i64) -> Result<String> {
        self.call(
            "getManagedBotToken",
            &serde_json::json!({ "user_id": user_id }),
        )
        .await
    }

    /// Long-poll the manager bot's update stream until a `managed_bot` update
    /// arrives, returning it (the new child's id is `bot.id`). Exponential
    /// backoff on transient errors; resets on each successful poll.
    pub async fn poll_for_managed_bot(&self) -> Result<ManagedBotUpdated> {
        let mut offset: i64 = 0;
        let mut backoff = BACKOFF_START;
        loop {
            match self.poll_once(offset).await {
                Ok((updates, next_offset)) => {
                    backoff = BACKOFF_START;
                    offset = next_offset;
                    for u in updates {
                        if let Some(managed) = u.managed_bot {
                            return Ok(managed);
                        }
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "managed-bot poll failed; backing off");
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff);
                }
            }
        }
    }

    /// One `getUpdates` cycle. Returns the updates and the next offset.
    async fn poll_once(&self, offset: i64) -> Result<(Vec<Update>, i64)> {
        let updates: Vec<Update> = self
            .call(
                "getUpdates",
                &serde_json::json!({
                    "offset": offset,
                    "timeout": LONG_POLL_SECS,
                    "allowed_updates": ["managed_bot"],
                }),
            )
            .await?;
        let next = updates
            .iter()
            .map(|u| u.update_id + 1)
            .max()
            .unwrap_or(offset);
        Ok((updates, next))
    }
}

/// Next backoff step: double, capped at [`BACKOFF_MAX`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(BACKOFF_MAX)
}

/// Resolve a token from an env var (trimmed; empty treated as missing).
pub fn token_from_env(var: &str) -> Result<String> {
    let val = std::env::var(var).map_err(|_| anyhow!("env var {var} is not set"))?;
    let trimmed = val.trim();
    if trimmed.is_empty() {
        bail!("env var {var} is empty");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_TOKEN: &str = "TEST:TOKEN";

    /// Build a client pointed at the mock server. Constructs the struct
    /// directly (private fields) so tests never touch the global
    /// `TEAMCTL_TG_API_BASE` env var — avoids races across parallel tests.
    fn client(base: String) -> ManagedBotClient {
        ManagedBotClient {
            http: reqwest::Client::new(),
            base,
            token: TEST_TOKEN.to_string(),
        }
    }

    /// Mock a successful call, asserting BOTH the request body (catches
    /// param-name/shape regressions) and returning `result` verbatim.
    async fn mock_ok(
        server: &MockServer,
        method_name: &str,
        expect_body: serde_json::Value,
        result: serde_json::Value,
    ) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TEST_TOKEN}/{method_name}")))
            .and(body_json(expect_body))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "ok": true, "result": result })),
            )
            .mount(server)
            .await;
    }

    async fn mock_err(server: &MockServer, method_name: &str, description: &str) {
        // Telegram surfaces application errors as HTTP 400 with `ok:false`.
        Mock::given(method("POST"))
            .and(path(format!("/bot{TEST_TOKEN}/{method_name}")))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "ok": false, "description": description })),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn get_managed_bot_token_success() {
        let server = MockServer::start().await;
        // Real wire shape: request `{ user_id }`, response a BARE string.
        mock_ok(
            &server,
            "getManagedBotToken",
            serde_json::json!({ "user_id": 42 }),
            serde_json::json!("123456:CHILD-TOKEN"),
        )
        .await;
        let token = client(server.uri())
            .get_managed_bot_token(42)
            .await
            .expect("token fetched");
        assert_eq!(token, "123456:CHILD-TOKEN");
    }

    #[tokio::test]
    async fn get_managed_bot_token_surfaces_api_error() {
        let server = MockServer::start().await;
        mock_err(&server, "getManagedBotToken", "Bad Request: bot not found").await;
        let err = client(server.uri())
            .get_managed_bot_token(42)
            .await
            .expect_err("api error surfaces");
        assert!(
            err.to_string().contains("bot not found"),
            "error should carry the description: {err}"
        );
    }

    #[tokio::test]
    async fn poll_returns_first_managed_bot_update() {
        let server = MockServer::start().await;
        mock_ok(
            &server,
            "getUpdates",
            serde_json::json!({
                "offset": 0,
                "timeout": 25,
                "allowed_updates": ["managed_bot"]
            }),
            serde_json::json!([{
                "update_id": 7,
                "managed_bot": {
                    "user": { "id": 100, "first_name": "Operator" },
                    "bot": { "id": 999, "username": "child_bot", "first_name": "Child" }
                }
            }]),
        )
        .await;
        let updated = client(server.uri())
            .poll_for_managed_bot()
            .await
            .expect("managed_bot update");
        assert_eq!(updated.bot.id, 999);
        assert_eq!(updated.user.id, 100);
    }

    #[tokio::test]
    async fn poll_once_surfaces_api_error() {
        let server = MockServer::start().await;
        mock_err(&server, "getUpdates", "Unauthorized").await;
        let err = client(server.uri())
            .poll_once(0)
            .await
            .expect_err("poll error surfaces");
        assert!(err.to_string().contains("Unauthorized"), "{err}");
    }

    #[test]
    fn next_backoff_doubles_then_caps() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(
            next_backoff(Duration::from_secs(8)),
            Duration::from_secs(16)
        );
        // Caps at BACKOFF_MAX (30s) rather than overshooting to 32s.
        assert_eq!(
            next_backoff(Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn creation_link_uses_t_me_newbot_form() {
        assert_eq!(
            ManagedBotClient::creation_link("teamctl_mgr_bot", "acme_sage_bot"),
            "https://t.me/newbot/teamctl_mgr_bot/acme_sage_bot"
        );
    }

    #[test]
    fn token_from_env_resolves_trimmed() {
        let var = "TEAMCTL_TEST_MANAGED_TOKEN_OK";
        std::env::set_var(var, "  abc:DEF  ");
        assert_eq!(token_from_env(var).unwrap(), "abc:DEF");
        std::env::remove_var(var);
    }

    #[test]
    fn token_from_env_rejects_missing_and_empty() {
        let missing = "TEAMCTL_TEST_MANAGED_TOKEN_MISSING";
        std::env::remove_var(missing);
        assert!(token_from_env(missing).is_err());

        let empty = "TEAMCTL_TEST_MANAGED_TOKEN_EMPTY";
        std::env::set_var(empty, "   ");
        assert!(token_from_env(empty).is_err());
        std::env::remove_var(empty);
    }
}
