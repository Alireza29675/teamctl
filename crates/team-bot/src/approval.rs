use std::sync::Arc;

use rusqlite::params;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::{
    repository::{decide_approval, should_route},
    State,
};

pub(super) fn approval_outcome_line(approved: bool, approver_first_name: &str) -> String {
    let verb = if approved {
        "✅ Approved"
    } else {
        "❌ Rejected"
    };
    format!("{verb} by {approver_first_name}")
}

/// #299: outcome line for a chosen multi-option decision. Names the
/// picked label so the card reads as a record of what was decided.
pub(super) fn decision_outcome_line(chosen_label: &str, approver_first_name: &str) -> String {
    format!("✅ {chosen_label} — chosen by {approver_first_name}")
}

/// #299: outcome line for the implicit Cancel. Distinct glyph + verb so
/// the operator (and the scrollback) can tell a cancel from a choice.
pub(super) fn cancel_outcome_line(approver_first_name: &str) -> String {
    format!("🚫 Cancelled by {approver_first_name}")
}

/// Parsed inline-button tap. `Approve`/`Deny` are the binary
/// back-compat card; `Opt(idx)`/`Cancel` are the #299 multi-option card.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CbAction {
    Approve,
    Deny,
    Opt(usize),
    Cancel,
}

/// Parse `callback_data` into `(approval_id, action)`. Formats:
/// `approve:{id}` / `deny:{id}` (binary), `opt:{id}:{idx}` /
/// `cancel:{id}` (multi-option). Returns `None` on anything malformed or
/// carrying trailing junk — `handle_callback` then ignores the tap, same
/// as the prior behavior for unrecognized data.
pub(super) fn parse_callback(data: &str) -> Option<(i64, CbAction)> {
    let mut it = data.split(':');
    let verb = it.next()?;
    let id: i64 = it.next()?.parse().ok()?;
    let action = match verb {
        "approve" => CbAction::Approve,
        "deny" => CbAction::Deny,
        "cancel" => CbAction::Cancel,
        "opt" => CbAction::Opt(it.next()?.parse().ok()?),
        _ => return None,
    };
    if it.next().is_some() {
        return None;
    }
    Some((id, action))
}

/// Decode the `options_json` column into `(label, value)` pairs. NULL or
/// malformed JSON yields empty → the card falls back to the binary
/// Approve/Deny render rather than failing the send (defensive: a
/// corrupt row should degrade, not wedge the outbound loop).
pub(super) fn decode_options(options_json: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = options_json else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .map(|arr| {
            arr.into_iter()
                .filter_map(|o| {
                    Some((
                        o.get("label")?.as_str()?.to_string(),
                        o.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the inline keyboard for an approval card. Empty `options` → the
/// binary Approve/Deny card (back-compat, byte-identical to the prior
/// render). Non-empty → one button per option (one per row; labels can
/// be long) followed by a Cancel row.
pub(super) fn approval_keyboard(id: i64, options: &[(String, String)]) -> InlineKeyboardMarkup {
    if options.is_empty() {
        return InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("Approve", format!("approve:{id}")),
            InlineKeyboardButton::callback("Deny", format!("deny:{id}")),
        ]]);
    }
    let mut rows: Vec<Vec<InlineKeyboardButton>> = options
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            vec![InlineKeyboardButton::callback(
                label.clone(),
                format!("opt:{id}:{i}"),
            )]
        })
        .collect();
    rows.push(vec![InlineKeyboardButton::callback(
        "Cancel",
        format!("cancel:{id}"),
    )]);
    InlineKeyboardMarkup::new(rows)
}

pub(super) async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    state: Arc<State>,
) -> ResponseResult<()> {
    let chat_id = q.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    if !state.is_authorized(chat_id) {
        return Ok(());
    }
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some((id, action)) = parse_callback(&data) else {
        return Ok(());
    };
    let route_ok = {
        let c = state.conn.lock().await;
        c.query_row(
            "SELECT agent_id FROM approvals WHERE id=?1",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .map(|agent| should_route(state.manager.as_deref(), &agent, &c))
        .unwrap_or(false)
    };
    if !route_ok {
        return Ok(());
    }

    // Resolve the tap to (terminal status, chosen value, outcome line,
    // toast glyph). `Opt` needs the immutable `options_json` from the row
    // to map idx→{label,value}; read it before the decision UPDATE (the
    // atomic WHERE status='pending' still guards against a concurrent
    // tap deciding between this read and the write).
    let (status, value, outcome, toast): (&str, Option<String>, String, String) = match action {
        CbAction::Approve => (
            "approved",
            None,
            approval_outcome_line(true, &q.from.first_name),
            format!("✅ #{id}"),
        ),
        CbAction::Deny => (
            "denied",
            None,
            approval_outcome_line(false, &q.from.first_name),
            format!("❌ #{id}"),
        ),
        CbAction::Cancel => (
            "denied",
            None,
            cancel_outcome_line(&q.from.first_name),
            format!("🚫 #{id}"),
        ),
        CbAction::Opt(idx) => {
            let opts = {
                let c = state.conn.lock().await;
                c.query_row(
                    "SELECT options_json FROM approvals WHERE id=?1",
                    params![id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
            };
            let decoded = decode_options(opts.as_deref());
            let Some((label, val)) = decoded.get(idx).cloned() else {
                // Out-of-range / corrupt options: don't decide, just
                // toast. The card stays tappable for a valid option.
                bot.answer_callback_query(q.id)
                    .text(format!("#{id} option unavailable"))
                    .await?;
                return Ok(());
            };
            (
                "decided",
                Some(val),
                decision_outcome_line(&label, &q.from.first_name),
                format!("✅ #{id}"),
            )
        }
    };

    // Atomic decision: only update if still pending. Returned row count tells
    // us whether this tap was the live decision or a stale duplicate.
    //
    // Order matters: status pin first, delivered_at flip second and
    // *only* when the status pin succeeded. The reverse order — flip
    // delivered_at unconditionally, then try the status pin — would
    // break the invariant `undeliverable ↔ delivered_at IS NULL` on
    // stale taps against rows that gc already moved to undeliverable.
    let decided_now = {
        let c = state.conn.lock().await;
        decide_approval(&c, id, status, value.as_deref()).unwrap_or_else(|error| {
            tracing::error!(%error, id, "approval decision failed");
            false
        })
    };

    if !decided_now {
        // Stale tap: row already terminal. Friendly toast, leave the message.
        bot.answer_callback_query(q.id)
            .text(format!("#{id} already resolved"))
            .await?;
        return Ok(());
    }

    // Live decision: edit the original message in-place to (a) append the
    // outcome line and (b) drop the inline buttons so the card can't be
    // re-clicked.
    if let Some(msg) = q.message.as_ref() {
        let chat = msg.chat().id;
        let mid = msg.id();
        let original = msg.regular_message().and_then(|m| m.text()).unwrap_or("");
        let new_text = if original.is_empty() {
            outcome.clone()
        } else {
            format!("{original}\n\n{outcome}")
        };
        let _ = bot.edit_message_text(chat, mid, new_text).await;
        let _ = bot
            .edit_message_reply_markup(chat, mid)
            .reply_markup(InlineKeyboardMarkup::new(Vec::<Vec<_>>::new()))
            .await;
    }

    bot.answer_callback_query(q.id).text(toast).await?;
    Ok(())
}
