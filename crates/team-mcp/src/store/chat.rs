//! Authorized human routing and atomic multipart replies at the MCP boundary.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use team_core::chat::{self, ChatMessage, Conversation, Repository};

use super::Store;

#[derive(Clone, serde::Serialize)]
pub(crate) struct ReplyPart {
    pub text: String,
    pub kind: &'static str,
    pub payload: Option<Value>,
}

#[derive(Clone, Copy)]
pub(crate) enum ControlMessage<'a> {
    Reaction {
        emoji: &'a str,
        telegram_message_id: Option<i64>,
    },
    Typing {
        expires_at: f64,
    },
}

pub(crate) enum ControlResult {
    Modern(Value),
    Legacy { telegram_message_id: Option<i64> },
}

enum Route {
    Modern(Conversation, Option<String>),
    Legacy(Option<i64>),
}

impl Store {
    pub(crate) fn chat_control(
        &self,
        agent: &str,
        conversation: Option<&str>,
        reply_to: Option<i64>,
        control: ControlMessage<'_>,
    ) -> Result<ControlResult> {
        let conn = self.connection()?;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        let repo = Repository::new(&tx);
        let (route, quoted) = match resolve_route(&tx, agent, conversation, reply_to, false)? {
            Route::Modern(route, quoted) => (route, quoted),
            Route::Legacy(stored) => {
                let external = match control {
                    ControlMessage::Reaction {
                        telegram_message_id,
                        ..
                    } => {
                        if reply_to.is_some() && stored.is_none() {
                            bail!("the addressed legacy message has no Telegram message ID");
                        }
                        if stored.zip(telegram_message_id).is_some_and(|(a, b)| a != b) {
                            return Err(chat::Error::Forbidden.into());
                        }
                        stored.or(telegram_message_id)
                    }
                    ControlMessage::Typing { .. } => stored,
                };
                return Ok(ControlResult::Legacy {
                    telegram_message_id: external,
                });
            }
        };
        team_core::interfaces::require_access(&tx, agent, &route.adapter_id)?;
        let (kind, text) = match control {
            ControlMessage::Reaction { emoji, .. } => ("reaction", emoji),
            ControlMessage::Typing { .. } => ("typing", ""),
        };
        let supported = repo
            .capabilities(&route.adapter_id)?
            .is_some_and(|caps| caps.iter().any(|cap| cap == kind));
        if !supported {
            return Ok(ControlResult::Modern(json!({
                "supported": false, "conversation_id": route.id
            })));
        }
        let payload = match control {
            ControlMessage::Typing { expires_at } => json!({"expires_at": expires_at}),
            ControlMessage::Reaction {
                emoji,
                telegram_message_id,
            } => {
                let telegram = route.adapter_id.starts_with("telegram:")
                    || team_core::interfaces::get(&tx, &route.adapter_id)?
                        .is_some_and(|i| i.provider == "telegram");
                if telegram && !crate::tools::is_allowed_reaction(emoji) {
                    bail!("react_to_user: emoji `{emoji}` is not in the bot-reaction allowlist");
                }
                let external = reaction_external(&tx, &route.id, quoted, telegram_message_id)?;
                let mut payload = json!({"emoji": emoji, "external_message_id": external});
                if telegram {
                    if let Ok(id) = external.parse::<i64>() {
                        payload["telegram_msg_id"] = json!(id);
                    }
                }
                payload
            }
        };
        let message = repo.reply(
            agent,
            Some(&route.id),
            reply_to,
            text,
            kind,
            Some(payload),
            None,
        )?;
        tx.commit()?;
        Ok(ControlResult::Modern(json!({
            "supported": true, "id": message.id, "conversation_id": route.id,
            "recipient": "human"
        })))
    }

    /// Validate local media only after authorizing the route. Exact successful
    /// retries return the durable receipt even if source files or inbox rows
    /// have since disappeared. A legacy result leaves validation to its caller.
    #[cfg(test)]
    pub(crate) fn reply_chat_validated(
        &self,
        agent: &str,
        conversation: Option<&str>,
        reply_to: Option<i64>,
        request: Option<&str>,
        parts: &[ReplyPart],
        validate: impl FnOnce() -> Result<()>,
    ) -> Result<Option<Vec<ChatMessage>>> {
        self.reply_chat_prepared(
            agent,
            conversation,
            reply_to,
            request,
            parts,
            validate,
            |_, _, _| Ok(()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reply_chat_prepared(
        &self,
        agent: &str,
        conversation: Option<&str>,
        reply_to: Option<i64>,
        request: Option<&str>,
        parts: &[ReplyPart],
        validate: impl FnOnce() -> Result<()>,
        prepare: impl FnOnce(&Connection, &Conversation, &mut Vec<ReplyPart>) -> Result<()>,
    ) -> Result<Option<Vec<ChatMessage>>> {
        let conn = self.connection()?;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        let fingerprint = blake3::hash(&serde_json::to_vec(&(conversation, reply_to, parts))?)
            .to_hex()
            .to_string();
        let receipt = cached_route(&tx, agent, request, &fingerprint)?;
        let cached = receipt.is_some();
        let selected = conversation.or(receipt.as_deref());
        let route = match resolve_route(
            &tx,
            agent,
            selected,
            if cached { None } else { reply_to },
            true,
        )? {
            Route::Modern(route, _) => route,
            Route::Legacy(_) => return Ok(None),
        };
        team_core::interfaces::require_access(&tx, agent, &route.adapter_id)?;
        let repo = Repository::new(&tx);
        if cached {
            let mut saved = Vec::with_capacity(parts.len());
            for index in 0..parts.len() {
                let id = request.map(|r| format!("{r}:{index}")).unwrap_or_default();
                let message_id = tx.query_row(
                    "SELECT message_id FROM chat_requests WHERE caller=?1 AND request_id=?2",
                    params![format!("agent:{agent}"), id],
                    |r| r.get::<_, i64>(0),
                )?;
                saved.push(repo.message(message_id)?);
            }
            tx.commit()?;
            return Ok(Some(saved));
        }
        let mut prepared = parts.to_vec();
        if !cached {
            validate_parts(&repo, &route, parts)?;
            validate()?;
            prepare(&tx, &route, &mut prepared)?;
        }
        let mut messages = Vec::with_capacity(parts.len());
        for (index, part) in prepared.iter().enumerate() {
            let id = request.map(|request| format!("{request}:{index}"));
            messages.push(repo.reply(
                agent,
                Some(&route.id),
                reply_to,
                &part.text,
                part.kind,
                part.payload.clone(),
                id.as_deref(),
            )?);
        }
        if let (Some(request), Some(first)) = (request, messages.first()) {
            tx.execute(
                "INSERT OR IGNORE INTO chat_requests(caller,request_id,fingerprint,message_id)
                VALUES(?1,?2,?3,?4)",
                params![format!("mcp:{agent}"), request, fingerprint, first.id],
            )?;
        }
        tx.commit()?;
        Ok(Some(messages))
    }

    #[cfg(test)]
    pub(crate) fn reply_chat(
        &self,
        agent: &str,
        conversation: Option<&str>,
        reply_to: Option<i64>,
        request: Option<&str>,
        parts: &[ReplyPart],
    ) -> Result<Option<Vec<ChatMessage>>> {
        self.reply_chat_validated(agent, conversation, reply_to, request, parts, || Ok(()))
    }
}

/// Preserve legacy routing only for a genuinely unlinked human message. A
/// half-linked or hidden row must never escape into the unaddressed queue.
fn resolve_route(
    conn: &Connection,
    agent: &str,
    conversation: Option<&str>,
    reply_to: Option<i64>,
    allow_stale_legacy_reply: bool,
) -> Result<Route> {
    if let Some(id) = reply_to {
        let row = conn
            .query_row(
                "SELECT sender, recipient, conversation_id, chat_message_id, kind, telegram_msg_id
             FROM messages WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()?;
        if let Some((sender, recipient, route, chat_id, kind, telegram_id)) = row {
            if recipient != agent
                || !sender.starts_with("user:")
                || kind.as_deref() == Some("media_pending")
            {
                return Err(chat::Error::Forbidden.into());
            }
            match (route, chat_id) {
                (None, None)
                    if conversation.is_none()
                        && team_core::interfaces::list_for_agent(conn, agent)?.is_empty() =>
                {
                    return Ok(Route::Legacy(telegram_id))
                }
                (Some(_), Some(_)) => {}
                _ => return Err(chat::Error::Forbidden.into()),
            }
        } else if conversation.is_none()
            && allow_stale_legacy_reply
            && !has_routes(conn, agent)?
            && team_core::interfaces::list_for_agent(conn, agent)?.is_empty()
        {
            // Historical reply_to_user tolerated stale threading hints when
            // no conversation existed. Controls require real addressed rows.
            return Ok(Route::Legacy(None));
        }
    }
    match Repository::new(conn).resolve_route(agent, conversation, reply_to) {
        Ok((route, quoted)) => Ok(Route::Modern(route, quoted)),
        Err(chat::Error::NoRoute)
            if conversation.is_none()
                && reply_to.is_none()
                && team_core::interfaces::list_for_agent(conn, agent)?.is_empty() =>
        {
            Ok(Route::Legacy(None))
        }
        Err(error) => Err(error.into()),
    }
}

fn cached_route(
    conn: &Connection,
    agent: &str,
    request: Option<&str>,
    fingerprint: &str,
) -> Result<Option<String>> {
    let Some(request) = request else {
        return Ok(None);
    };
    if request.is_empty() || request.len() > 1000 || request.chars().any(char::is_control) {
        bail!("request_id must contain 1..1000 bytes and no control characters");
    }
    let saved: Option<(String, String)> = conn
        .query_row(
            "SELECT r.fingerprint,m.conversation_id FROM chat_requests r
         JOIN chat_messages m ON m.id=r.message_id WHERE r.caller=?1 AND r.request_id=?2",
            params![format!("mcp:{agent}"), request],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match saved {
        Some((stored, _)) if stored != fingerprint => Err(chat::Error::IdempotencyConflict.into()),
        Some((_, conversation)) => Ok(Some(conversation)),
        None => Ok(None),
    }
}

fn validate_parts(repo: &Repository<'_>, route: &Conversation, parts: &[ReplyPart]) -> Result<()> {
    if let Some(capabilities) = repo.capabilities(&route.adapter_id)? {
        for part in parts {
            if !capabilities
                .iter()
                .any(|capability| capability == part.kind)
            {
                bail!("the conversation adapter does not support {}", part.kind);
            }
        }
    } else if parts.iter().any(|part| part.kind != "text") {
        bail!("adapter capabilities are not available; connect it before sending non-text content");
    }
    if route.adapter_id.starts_with("telegram:")
        || team_core::interfaces::get(repo.connection(), &route.adapter_id)?
            .is_some_and(|i| i.provider == "telegram")
    {
        for part in parts {
            let limit = if part.kind == "text" { 4096 } else { 1024 };
            if part.text.encode_utf16().count() > limit {
                bail!("reply_to_user: content exceeds Telegram's {limit}-character limit; split it into smaller replies");
            }
        }
    }
    Ok(())
}

fn reaction_external(
    conn: &Connection,
    conversation: &str,
    quoted: Option<String>,
    telegram: Option<i64>,
) -> Result<String> {
    if let Some(quoted) = quoted {
        if telegram.is_some_and(|id| id.to_string() != quoted) {
            return Err(chat::Error::Forbidden.into());
        }
        return Ok(quoted);
    }
    let external = telegram
        .ok_or_else(|| anyhow::anyhow!("reaction needs an addressed inbound message"))?
        .to_string();
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE conversation_id=?1
         AND direction='human' AND external_id=?2)",
        params![conversation, external],
        |r| r.get(0),
    )?;
    if !allowed {
        return Err(chat::Error::Forbidden.into());
    }
    Ok(external)
}

fn has_routes(conn: &Connection, agent: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_conversations WHERE agent_id=?1)",
        [agent],
        |r| r.get(0),
    )?)
}

impl Store {
    pub(crate) fn list_interfaces(
        &self,
        agent: &str,
    ) -> Result<Vec<team_core::interfaces::InterfaceInfo>> {
        let conn = self.connection()?;
        Ok(team_core::interfaces::list_for_agent(&conn, agent)?)
    }

    pub(crate) fn select_interface(&self, agent: &str, interface: &str) -> Result<String> {
        let conn = self.connection()?;
        if !team_core::interfaces::is_allowed(&conn, agent, interface)? {
            return Err(chat::Error::Forbidden.into());
        }
        let mut stmt = conn.prepare(
            "SELECT id FROM chat_conversations WHERE agent_id=?1 AND adapter_id=?2 LIMIT 2",
        )?;
        let ids = stmt
            .query_map(params![agent, interface], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] if team_core::interfaces::get(&conn, interface)?
                .is_some_and(|i| i.provider == "web") =>
            {
                Ok(Repository::new(&conn)
                    .open_interface(agent, interface, "operator", agent)?
                    .id)
            }
            [] => Err(chat::Error::NoRoute.into()),
            _ => Err(chat::Error::AmbiguousRoute.into()),
        }
    }
}
