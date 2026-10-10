use serde::Deserialize;
use serde_json::{json, Value};

use super::{content_json, Ctx};

/// T-32b: read an operator-attached file via the broker policy.
/// The agent passes the path it found in a message body's
/// `📎 attachment: <path>` marker; the broker canonicalizes,
/// path-traversal-checks against `allowed_roots`, size-checks
/// against `max_size_bytes`, runs the configured scanner if any,
/// stages the bytes to a content-addressed tempfile, audit-logs the
/// attempt, and returns the staging path. Rejects fan out a
/// notification both to telegram (`recipient = 'user:telegram'`)
/// and to the project's `all` channel (TUI Wire tab) so the
/// operator sees the reason regardless of which surface they're on.
#[derive(Deserialize)]
pub(super) struct ReadAttachmentArgs {
    path: String,
}

pub(super) async fn read_attachment(ctx: &Ctx, args: Value) -> Result<Value, String> {
    use std::path::PathBuf;
    use team_core::attachments::{
        append_audit, check_and_read_with_metadata, now_rfc3339, stage_to_tempfile, staging_dir,
        AuditEntry, RejectReason,
    };

    let a: ReadAttachmentArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
    let raw_path = PathBuf::from(&a.path);

    // Without compose context we can't enforce policy or stage.
    // Hand-launched team-mcp instances see this — surface as a
    // disabled-style reject so the agent doesn't think they're
    // getting bytes.
    let Some(att) = ctx.attachments.as_ref() else {
        return Ok(content_json(&json!({
            "rejected": true,
            "reason": "attachments unavailable: team-mcp launched without --compose-root",
        })));
    };

    let cfg = &att.cfg;
    let scanner_ref: Option<&dyn team_core::attachments::Scanner> = att.scanner.as_deref();

    let outcome = check_and_read_with_metadata(cfg, &raw_path, scanner_ref);
    let audit_path = cfg
        .audit_log_path
        .as_ref()
        .map(|p| resolve_audit_path(&att.compose_root, p));

    match outcome {
        Ok(accepted) => {
            let staged = stage_to_tempfile(&staging_dir(&att.compose_root), &accepted)
                .map_err(|e| format!("stage_to_tempfile: {e}"))?;
            // Audit on accept.
            let entry = AuditEntry {
                ts: now_rfc3339(),
                path: &a.path,
                resolved: accepted.resolved.to_str(),
                outcome: "accept",
                size: Some(accepted.size),
                blake3: Some(&accepted.blake3_hex),
                reason: None,
            };
            if let Err(e) = append_audit(audit_path.as_deref(), &entry) {
                tracing::warn!(error = %e, "audit log append failed (accept)");
            }
            Ok(content_json(&json!({
                "rejected": false,
                "temp_path": staged.display().to_string(),
                "blake3": accepted.blake3_hex,
                "size": accepted.size,
            })))
        }
        Err(reason) => {
            // Audit on reject. Resolved path may be unknown
            // (path-unresolvable / outside-roots variants both
            // shape it differently); pull what we have.
            let resolved_owned = match &reason {
                RejectReason::OutsideAllowedRoots { resolved } => {
                    resolved.to_str().map(|s| s.to_string())
                }
                _ => None,
            };
            let human = reason.human();
            let entry = AuditEntry {
                ts: now_rfc3339(),
                path: &a.path,
                resolved: resolved_owned.as_deref(),
                outcome: "reject",
                size: None,
                blake3: None,
                reason: Some(human.clone()),
            };
            if let Err(e) = append_audit(audit_path.as_deref(), &entry) {
                tracing::warn!(error = %e, "audit log append failed (reject)");
            }

            // Reject notification: telegram + project-wide Wire
            // (channel:<project>:all). Both rows go through the
            // existing send_dm path so team-bot's outbound loop and
            // the TUI's Wire-tab query both see them without new
            // wiring.
            let notice = format!("📎 broker rejected attachment {}: {human}", a.path);
            let project = ctx.project();
            let broker_id = format!("{project}:broker");
            let _ = ctx
                .store
                .send_dm(project, &ctx.agent_id, "user:telegram", &notice, None, None);
            let wire_recipient = format!("channel:{project}:all");
            let _ = ctx
                .store
                .send_dm(project, &broker_id, &wire_recipient, &notice, None, None);

            Ok(content_json(&json!({
                "rejected": true,
                "reason": human,
            })))
        }
    }
}

/// Resolve an `audit_log_path` from compose against the compose
/// root: relative paths join, absolute paths pass through.
pub(super) fn resolve_audit_path(
    compose_root: &std::path::Path,
    p: &std::path::Path,
) -> std::path::PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        compose_root.join(p)
    }
}
