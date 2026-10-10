use serde::Deserialize;
use serde_json::{json, Value};

use super::{content_json, Ctx};

#[derive(Deserialize)]
pub(super) struct DmArgs {
    to: String,
    text: String,
    #[serde(default)]
    thread_id: Option<String>,
}

pub(super) async fn dm(ctx: &Ctx, args: Value) -> Result<Value, String> {
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
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok(content_json(&json!({ "id": id, "recipient": recipient })))
}

#[derive(Deserialize)]
pub(super) struct BroadcastArgs {
    channel: String,
    text: String,
}

pub(super) fn broadcast(ctx: &Ctx, args: Value) -> Result<Value, String> {
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
