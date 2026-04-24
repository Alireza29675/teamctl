//! MCP tool definitions and dispatch.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::store::Store;

pub struct Ctx {
    pub agent_id: String,
    pub store: Arc<Store>,
}

impl Ctx {
    pub fn new(agent_id: String, store: Store) -> Self {
        Self {
            agent_id,
            store: Arc::new(store),
        }
    }

    pub fn project(&self) -> &str {
        self.agent_id.split(':').next().unwrap_or("")
    }
}

/// JSON-Schema-ish tool list for `tools/list`.
pub fn schema() -> Value {
    json!([
        {
            "name": "whoami",
            "description": "Return the caller's fully-qualified agent id.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "dm",
            "description": "Send a direct message to another agent (same project). Returns the new message id.",
            "inputSchema": {
                "type": "object",
                "required": ["to", "text"],
                "properties": {
                    "to":        { "type": "string", "description": "Target agent id. Either `<project>:<agent>` or a bare `<agent>` in the caller's project." },
                    "text":      { "type": "string" },
                    "thread_id": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_peek",
            "description": "Return up to `limit` unacked messages addressed to the caller. Non-destructive.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_ack",
            "description": "Mark the listed message ids as acknowledged so they stop appearing in inbox_peek/inbox_watch.",
            "inputSchema": {
                "type": "object",
                "required": ["ids"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_watch",
            "description": "Block up to `timeout_ms` milliseconds waiting for a new message. Returns immediately if any are pending.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 60000, "default": 15000 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "broadcast",
            "description": "Post a message to a channel in the caller's project. Caller must be a channel member and have the channel listed in can_broadcast.",
            "inputSchema": {
                "type": "object",
                "required": ["channel", "text"],
                "properties": {
                    "channel": { "type": "string" },
                    "text":    { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "list_team",
            "description": "List every agent in the caller's project (project-scoped; never returns other projects).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }
    ])
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
        "inbox_ack" => inbox_ack(ctx, p.arguments),
        "inbox_watch" => inbox_watch(ctx, p.arguments).await,
        "broadcast" => broadcast(ctx, p.arguments),
        "list_team" => list_team(ctx),
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

#[derive(Deserialize)]
struct DmArgs {
    to: String,
    text: String,
    #[serde(default)]
    thread_id: Option<String>,
}

async fn dm(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: DmArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    // Resolve bare `<agent>` as `<self-project>:<agent>`.
    let recipient = if a.to.contains(':') {
        a.to.clone()
    } else {
        format!("{}:{}", ctx.project(), a.to)
    };
    // Project isolation: DM recipient must be in the same project as caller.
    let caller_project = ctx.project().to_string();
    let recipient_project = recipient.split(':').next().unwrap_or_default().to_string();
    if recipient_project != caller_project {
        // Cross-project: only allowed when a live bridge authorizes it.
        match ctx
            .store
            .live_bridge(&ctx.agent_id, &recipient)
            .map_err(|e| e.to_string())?
        {
            Some(_bridge_id) => {
                // Permitted. Thread-id is used by `teamctl bridge log` to
                // reconstruct the transcript.
            }
            None => {
                return Err(format!(
                    "project isolation: cannot DM across projects ({caller_project} -> {recipient_project}); open a bridge",
                ));
            }
        }
    }
    // ACL: `can_dm` must include the recipient (or be empty = unrestricted).
    if !ctx
        .store
        .can_dm(&ctx.agent_id, &recipient)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {sender} is not permitted to DM {recipient}",
            sender = ctx.agent_id
        ));
    }
    // If this is a bridged DM, record the bridge id in thread_id for auditing.
    let bridge_thread = if recipient_project != caller_project {
        ctx.store
            .live_bridge(&ctx.agent_id, &recipient)
            .ok()
            .flatten()
            .map(|id| format!("bridge:{id}"))
    } else {
        None
    };
    let thread_id = bridge_thread.as_deref().or(a.thread_id.as_deref());
    let id = ctx
        .store
        .send_dm(
            &caller_project,
            &ctx.agent_id,
            &recipient,
            &a.text,
            thread_id,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "recipient": recipient })))
}

#[derive(Deserialize)]
struct BroadcastArgs {
    channel: String,
    text: String,
}

fn broadcast(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: BroadcastArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let project = ctx.project();
    if !ctx
        .store
        .is_channel_member(project, &a.channel, &ctx.agent_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {agent} is not a member of channel {channel} in project {project}",
            agent = ctx.agent_id,
            channel = a.channel,
        ));
    }
    if !ctx
        .store
        .can_broadcast(&ctx.agent_id, &a.channel)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "ACL: {agent} is not permitted to broadcast on {channel}",
            agent = ctx.agent_id,
            channel = a.channel,
        ));
    }
    let id = ctx
        .store
        .send_broadcast(project, &ctx.agent_id, &a.channel, &a.text)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "channel": a.channel })))
}

#[derive(Deserialize, Default)]
struct InboxPeekArgs {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    20
}

fn inbox_peek(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxPeekArgs = if args.is_null() {
        InboxPeekArgs::default()
    } else {
        serde_json::from_value(args).map_err(|e| e.to_string())?
    };
    let msgs = ctx
        .store
        .inbox_peek(&ctx.agent_id, a.limit)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "messages": msgs })))
}

#[derive(Deserialize)]
struct InboxAckArgs {
    ids: Vec<i64>,
}

fn inbox_ack(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxAckArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let n = ctx.store.inbox_ack(&a.ids).map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "acked": n })))
}

#[derive(Deserialize, Default)]
struct InboxWatchArgs {
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
fn default_timeout() -> u64 {
    15000
}

async fn inbox_watch(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxWatchArgs = if args.is_null() {
        InboxWatchArgs::default()
    } else {
        serde_json::from_value(args).map_err(|e| e.to_string())?
    };
    // Poll every 250 ms up to the deadline. Simple, good enough for Phase 1.
    let mut remaining = a.timeout_ms;
    loop {
        let msgs = ctx
            .store
            .inbox_peek(&ctx.agent_id, 20)
            .map_err(|e| e.to_string())?;
        if !msgs.is_empty() || remaining == 0 {
            return Ok(content_json(&json!({ "messages": msgs })));
        }
        let step = remaining.min(250);
        sleep(Duration::from_millis(step)).await;
        remaining -= step;
    }
}

fn list_team(ctx: &Ctx) -> Result<Value, String> {
    let ids = ctx
        .store
        .list_project_agents(ctx.project())
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "agents": ids })))
}
