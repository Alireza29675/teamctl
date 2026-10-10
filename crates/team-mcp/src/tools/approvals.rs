use serde::Deserialize;
use serde_json::{json, Value};

use super::{content_json, Ctx};
use crate::store::ApprovalStatus;

#[derive(Deserialize)]
pub(super) struct ApprovalArgs {
    action: String,
    #[serde(default)]
    scope_tag: Option<String>,
    summary: String,
    #[serde(default)]
    payload: Value,
    #[serde(default = "default_approval_ttl")]
    ttl_seconds: u64,
    #[serde(default = "default_approval_wait")]
    wait: bool,
    /// #299: labeled options for a multi-choice decision. When absent the
    /// prompt renders as the binary Approve/Deny card (back-compat). When
    /// present the bot renders one button per option plus an implicit
    /// Cancel; the chosen `value` comes back to the agent.
    #[serde(default)]
    options: Option<Vec<ApprovalOption>>,
}

#[derive(Deserialize)]
pub(super) struct ApprovalOption {
    label: String,
    value: String,
}
pub(super) fn default_approval_ttl() -> u64 {
    900
}
pub(super) fn default_approval_wait() -> bool {
    true
}

pub(super) async fn request_approval(ctx: &Ctx, args: Value) -> Result<Value, String> {
    let a: ApprovalArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(a.ttl_seconds))
        .ok_or_else(|| "request_approval: ttl_seconds is too large".to_string())?;
    let payload_str = serde_json::to_string(&a.payload).unwrap_or_else(|_| "{}".into());
    // Serialize options to the `options_json` column the bot reads to
    // render N buttons. `None` (binary callers) leaves the column NULL →
    // the bot takes the Approve/Deny back-compat path.
    let options_json = match &a.options {
        Some(opts) if !opts.is_empty() => Some(
            serde_json::to_string(
                &opts
                    .iter()
                    .map(|o| json!({ "label": o.label, "value": o.value }))
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| e.to_string())?,
        ),
        _ => None,
    };
    let id = ctx
        .store
        .request_approval(
            ctx.project(),
            &ctx.agent_id,
            &a.action,
            a.scope_tag.as_deref(),
            &a.summary,
            &payload_str,
            a.ttl_seconds as f64,
            options_json.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    if !a.wait {
        let ApprovalStatus {
            status,
            note,
            value,
            delivered_at,
        } = ctx.store.approval_status(id).map_err(|e| e.to_string())?;
        return Ok(content_json(&json!({
            "id": id,
            "status": status,
            "note": note,
            "value": value,
            "delivered_at": delivered_at,
        })));
    }

    // Poll every 500 ms until decided or expired.
    loop {
        let _ = ctx.store.expire_stale_approvals();
        let ApprovalStatus {
            status,
            note,
            value,
            delivered_at,
        } = ctx.store.approval_status(id).map_err(|e| e.to_string())?;
        if status != "pending" {
            return Ok(content_json(&json!({
                "id": id,
                "status": status,
                "note": note,
                "value": value,
                "delivered_at": delivered_at,
            })));
        }
        if std::time::Instant::now() >= deadline {
            // Force-expire one last time.
            let _ = ctx.store.expire_stale_approvals();
            let ApprovalStatus {
                status,
                note,
                value,
                delivered_at,
            } = ctx.store.approval_status(id).map_err(|e| e.to_string())?;
            return Ok(content_json(&json!({
                "id": id,
                "status": status,
                "note": note,
                "value": value,
                "delivered_at": delivered_at,
            })));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
