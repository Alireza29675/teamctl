//! Minimal JSON-RPC 2.0 + MCP dispatch layer.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::{self, Ctx};

#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
    pub fn parse_error(msg: &str) -> Self {
        Self::err(Value::Null, -32700, format!("parse error: {msg}"))
    }
}

pub async fn dispatch(ctx: &Ctx, req: Request) -> Option<Response> {
    let id = req.id.clone().unwrap_or(Value::Null);
    if req.jsonrpc != "2.0" {
        return Some(Response::err(id, -32600, "jsonrpc must be 2.0"));
    }
    // Notifications (no id) get no response.
    let is_notification = req.id.is_none();

    let result = match req.method.as_str() {
        // The `experimental.claude/channel` capability is what registers
        // the channel listener on the Claude Code side; without it the
        // runtime silently drops every `notifications/claude/channel`
        // we emit. `serverInfo.name` becomes the `source=` attribute on
        // the rendered `<channel source="team">` tag, so it must match
        // both the `.mcp.json` key the wrapper renders and the bootstrap
        // prompt the agent reads.
        "initialize" => Ok(json!({
            "protocolVersion": team_core::MCP_PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false },
                "experimental": { "claude/channel": {} },
            },
            "serverInfo": { "name": "team", "version": team_core::VERSION },
            "instructions": "Team traffic arrives as <channel source=\"team\"> events with meta attributes id, sender, recipient, thread_id, sent_at. By default the body is a short \"📬 1 new message …\" stub (meta.lazy=\"1\") — call inbox_read with the meta.id to fetch the full body and resolve it in one step. If the stub doesn't merit handling, call inbox_ack with the id to dismiss it. When the body lands inline (no meta.lazy, e.g. operator used /readnow), act on it directly and call inbox_ack on the id when done. Do not poll inbox_watch — channels are push-driven. Use inbox_peek only for non-destructive catch-up after a restart.",
        })),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(json!({ "tools": tools::schema() })),
        "tools/call" => tools::call(ctx, req.params).await,
        "ping" => Ok(json!({})),
        other => Err(format!("method not found: {other}")),
    };

    if is_notification {
        return None;
    }
    match result {
        Ok(v) => Some(Response::ok(id, v)),
        Err(e) => Some(Response::err(id, -32601, e)),
    }
}
