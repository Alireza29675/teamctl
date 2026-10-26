//! MCP tool definitions and dispatch.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::store::Store;
use std::path::PathBuf;
use team_core::attachments::Scanner;
use team_core::compose::Attachments;

mod schema;
pub use schema::schema;
mod messaging;
use messaging::*;
mod human;
pub(crate) use human::is_allowed_reaction;
use human::*;
mod attachments;
use attachments::*;
mod tmux;
use tmux::*;
pub(crate) use tmux::{pane_session, tmux_type_and_submit};
mod inbox;
use inbox::*;
mod approvals;
use approvals::*;

pub struct Ctx {
    pub agent_id: String,
    pub store: Arc<Store>,
    /// Tmux session prefix (matches `compose.global.supervisor.tmux_prefix`).
    /// Used by `compact_self` to compute the caller's tmux session name
    /// when sending the `/compact` slash command into its pane.
    pub tmux_prefix: String,
    /// T-32b: attachment policy + compose root. `None` when team-mcp
    /// was launched without `--compose-root` (hand-launched servers,
    /// older renderer); `read_attachment` returns "disabled" in that
    /// case so an unconfigured server doesn't silently expose the
    /// filesystem.
    pub attachments: Option<AttachmentsCtx>,
}

/// Bundles the attachment-related state on `Ctx` so the tool body
/// stays parameter-light. The scanner is constructed once at boot
/// and re-used across calls.
pub struct AttachmentsCtx {
    pub cfg: Attachments,
    pub compose_root: PathBuf,
    pub scanner: Option<Box<dyn Scanner>>,
}

impl Ctx {
    pub fn new(agent_id: String, store: Store, tmux_prefix: String) -> Self {
        Self {
            agent_id,
            store: Arc::new(store),
            tmux_prefix,
            attachments: None,
        }
    }

    pub fn with_attachments(mut self, attachments: AttachmentsCtx) -> Self {
        self.attachments = Some(attachments);
        self
    }

    pub fn project(&self) -> &str {
        self.agent_id.split(':').next().unwrap_or("")
    }
}

/// Convenience for tests: build a `Ctx` with an explicit compose
/// root + attachments cfg. Production wiring goes through `main.rs`.
#[cfg(test)]
impl Ctx {
    pub fn for_test_with_attachments(
        agent_id: String,
        store: Store,
        compose_root: &std::path::Path,
        cfg: Attachments,
        scanner: Option<Box<dyn Scanner>>,
    ) -> Self {
        Self::new(agent_id, store, "t-".into()).with_attachments(AttachmentsCtx {
            cfg,
            compose_root: compose_root.to_path_buf(),
            scanner,
        })
    }
}

#[derive(Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

pub async fn call(ctx: &Ctx, params: Value) -> Result<Value, String> {
    let p: CallParams = serde_json::from_value(params).map_err(|e| e.to_string())?;
    match p.name.as_str() {
        "whoami" => Ok(content_text(&ctx.agent_id)),
        "dm" => dm(ctx, p.arguments).await,
        "inbox_peek" => inbox_peek(ctx, p.arguments),
        "inbox_read" => inbox_read(ctx, p.arguments),
        "inbox_ack" => inbox_ack(ctx, p.arguments),
        "inbox_watch" => inbox_watch(ctx, p.arguments).await,
        "broadcast" => broadcast(ctx, p.arguments),
        "list_team" => list_team(ctx),
        "org_chart" => org_chart(ctx),
        "request_approval" => request_approval(ctx, p.arguments).await,
        "reply_to_user" => reply_to_user(ctx, p.arguments).await,
        "react_to_user" => react_to_user(ctx, p.arguments).await,
        "show_typing" => show_typing(ctx, p.arguments).await,
        "read_attachment" => read_attachment(ctx, p.arguments).await,
        "compact_self" => compact_self(ctx).await,
        other => Err(format!("unknown tool: {other}")),
    }
}

fn content_text(s: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": s } ], "isError": false })
}

fn content_json(v: &Value) -> Value {
    json!({
        "content": [
            { "type": "text", "text": serde_json::to_string(v).unwrap_or_default() }
        ],
        "isError": false,
        "structuredContent": v,
    })
}

#[cfg(test)]
mod tests;
