use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::sleep;

use super::{content_json, Ctx};

#[derive(Deserialize, Default)]
pub(super) struct InboxPeekArgs {
    #[serde(default = "default_limit")]
    limit: usize,
}
pub(super) fn default_limit() -> usize {
    20
}

pub(super) fn inbox_peek(ctx: &Ctx, args: Value) -> Result<Value, String> {
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
pub(super) struct InboxAckArgs {
    ids: Vec<i64>,
}

pub(super) fn inbox_ack(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxAckArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let n = ctx
        .store
        .inbox_ack(&ctx.agent_id, &a.ids)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "acked": n })))
}

#[derive(Deserialize)]
pub(super) struct InboxReadArgs {
    ids: Vec<i64>,
}

pub(super) fn inbox_read(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxReadArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let msgs = ctx
        .store
        .inbox_read(&ctx.agent_id, &a.ids)
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "messages": msgs })))
}

#[derive(Deserialize, Default)]
pub(super) struct InboxWatchArgs {
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
pub(super) fn default_timeout() -> u64 {
    15000
}

pub(super) async fn inbox_watch(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: InboxWatchArgs = if args.is_null() {
        InboxWatchArgs::default()
    } else {
        serde_json::from_value(args).map_err(|e| e.to_string())?
    };
    // Poll every 250 ms up to the deadline.
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

pub(super) fn list_team(ctx: &Ctx) -> Result<Value, String> {
    let ids = ctx
        .store
        .list_project_agents(ctx.project())
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "agents": ids })))
}

pub(super) fn org_chart(ctx: &Ctx) -> Result<Value, String> {
    let v = ctx
        .store
        .org_chart(ctx.project())
        .map_err(|e| e.to_string())?;
    Ok(content_json(&v))
}
