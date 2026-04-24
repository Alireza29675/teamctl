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
        "initialize" => Ok(json!({
            "protocolVersion": team_core::MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "team-mcp", "version": team_core::VERSION },
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
