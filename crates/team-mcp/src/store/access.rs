use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Store;

impl Store {
    /// Return the id of a currently-live bridge linking `from` and `to` (in
    /// either direction), or `None` if no live bridge authorizes the DM.
    pub fn live_bridge(&self, from: &str, to: &str) -> Result<Option<i64>> {
        let now = Self::now();
        let conn = self.connection()?;
        let row: Option<i64> = conn
            .query_row(
                "SELECT id FROM bridges
                 WHERE closed_at IS NULL
                   AND expires_at > ?1
                   AND ((from_agent = ?2 AND to_agent = ?3)
                     OR (from_agent = ?3 AND to_agent = ?2))
                 LIMIT 1",
                params![now, from, to],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    /// Does `agent_id` have permission to DM `recipient_agent_id`?
    /// An empty `can_dm` list means unrestricted (any same-project agent).
    pub fn can_dm(&self, agent_id: &str, recipient_agent_id: &str) -> Result<bool> {
        let conn = self.connection()?;
        let row: Option<String> = conn
            .query_row(
                "SELECT can_dm_json FROM agent_acls WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json) = row else {
            return Ok(true); // no ACL row = unrestricted
        };
        let allowed: Vec<String> =
            serde_json::from_str(&json).context("invalid direct-message ACL")?;
        if allowed.is_empty() {
            return Ok(true);
        }
        let short = recipient_agent_id
            .split_once(':')
            .map(|(_, a)| a)
            .unwrap_or(recipient_agent_id);
        Ok(allowed
            .iter()
            .any(|a| a == short || a == recipient_agent_id))
    }

    /// Does `agent_id` have permission to post to channel `channel_name` in its project?
    pub fn can_broadcast(&self, agent_id: &str, channel_name: &str) -> Result<bool> {
        let conn = self.connection()?;
        let row: Option<String> = conn
            .query_row(
                "SELECT can_bcast_json FROM agent_acls WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json) = row else {
            return Ok(true);
        };
        let allowed: Vec<String> = serde_json::from_str(&json).context("invalid broadcast ACL")?;
        if allowed.is_empty() {
            return Ok(true);
        }
        Ok(allowed.iter().any(|c| c == channel_name))
    }

    /// Is `agent_id` a member of `channel_name` in its project?
    pub fn is_channel_member(
        &self,
        project: &str,
        channel_name: &str,
        agent_id: &str,
    ) -> Result<bool> {
        let conn = self.connection()?;
        let cid = format!("{project}:{channel_name}");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND agent_id = ?2",
            params![cid, agent_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }
}
