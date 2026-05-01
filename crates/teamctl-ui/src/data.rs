//! `TeamSnapshot` — point-in-time read of the dogfood team that the UI
//! renders against. Built by walking up to the nearest `.team/`,
//! parsing `team-compose.yaml`, querying the supervisor for each
//! agent's process state, and aggregating a small set of mailbox
//! counters (unread + pending approvals).
//!
//! Read by both `App::tick()` (live refresh every second) and the
//! snapshot tests (constructed manually). The snapshot is intentionally
//! cheap to build — every field is derived from a single SQL query
//! per agent — so refresh cadence stays well under tmux's own
//! `capture-pane` cost.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use team_core::compose::Compose;
use team_core::supervisor::{AgentSpec, AgentState, Supervisor, TmuxSupervisor};

/// Per-agent fields the UI reads to render the roster + drive
/// selection / detail-pane streaming.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// `<project>:<agent>` — the canonical id used in `teamctl send`
    /// targets, MCP tool calls, and `reports_to` chains.
    pub id: String,
    /// Short agent name within the project (the YAML key).
    pub agent: String,
    /// Project id this agent belongs to.
    pub project: String,
    /// Resolved tmux session name (`<prefix><project>-<agent>`) — fed
    /// to the pane-capture call so the detail pane targets the right
    /// session even when `tmux_prefix` rotates.
    pub tmux_session: String,
    /// Process state — `Running`, `Stopped`, or `Unknown` per the
    /// supervisor trait. Drives the primary glyph in the roster.
    pub state: AgentState,
    /// Count of mailbox messages addressed to this agent that haven't
    /// been ack'd yet. Surfaces the `✉` glyph when nonzero.
    pub unread_mail: u32,
    /// Count of `request_approval` rows still in `pending` state for
    /// this agent. Surfaces the `!` glyph when nonzero (highest
    /// priority — overrides the unread-mail glyph).
    pub pending_approvals: u32,
    /// `true` for managers (`is_manager: true` in compose), used when
    /// the roster wants to draw a tier separator. Read but unused in
    /// PR-UI-2; kept on the struct so PR-UI-4's approvals modal can
    /// route based on tier without a second compose lookup.
    pub is_manager: bool,
}

/// One channel exposed in `team-compose.yaml`. Used by PR-UI-6's
/// per-channel broadcast picker and by the Mailbox-first layout's
/// channel list. `id` is `<project>:<name>` (matches the broker's
/// `channels.id`); `name` is the short label rendered as `#name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub project_id: String,
}

#[derive(Debug, Clone)]
pub struct TeamSnapshot {
    /// Path to the `.team/` discovered by walk-up (the compose root).
    pub root: PathBuf,
    /// Human label from `team-compose.yaml::projects[].project.name`
    /// — falls back to the project id when name is empty.
    pub team_name: String,
    /// Agents in deterministic order: managers first, then workers,
    /// each group sorted by id. Roster navigation (`↑` / `↓`) walks
    /// this slice directly.
    pub agents: Vec<AgentInfo>,
    /// Channels declared across every project file. Drives the
    /// PR-UI-6 broadcast picker + the Mailbox-first layout's
    /// channel list.
    pub channels: Vec<ChannelInfo>,
}

impl TeamSnapshot {
    /// Build an empty snapshot rooted at the given path. Used by
    /// tests and as the rendered shape when no `.team/` is reachable.
    pub fn empty(root: PathBuf) -> Self {
        Self {
            root,
            team_name: "(no team loaded)".into(),
            agents: Vec::new(),
            channels: Vec::new(),
        }
    }

    /// Walk up from cwd to find the nearest `.team/`, parse the
    /// compose tree, query supervisor + mailbox state per agent,
    /// and return the assembled snapshot. Returns `Ok(None)` when
    /// no `.team/` is reachable — the UI renders the empty state in
    /// that case rather than panicking.
    pub fn discover_and_load() -> Result<Option<Self>> {
        let cwd = std::env::current_dir().context("get cwd")?;
        match Compose::discover(&cwd) {
            Ok(root) => Self::load(&root).map(Some),
            Err(_) => Ok(None),
        }
    }

    /// Build a snapshot for an explicit `.team/` root. Public so
    /// integration tests can hand-feed a tempdir without going
    /// through walk-up discovery.
    pub fn load(root: &Path) -> Result<Self> {
        let compose = Compose::load(root)?;
        let mailbox = compose.root.join(&compose.global.broker.path);
        let counts = mailbox_counts(&mailbox).unwrap_or_default();

        let supervisor = TmuxSupervisor;
        let team_name = compose
            .projects
            .first()
            .map(|p| {
                if p.project.name.is_empty() {
                    p.project.id.clone()
                } else {
                    p.project.name.clone()
                }
            })
            .unwrap_or_else(|| "(unnamed team)".into());

        let mut agents = Vec::new();
        for h in compose.agents() {
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            let state = supervisor.state(&spec).unwrap_or(AgentState::Unknown);
            let id = h.id();
            let unread_mail = counts.unread.get(&id).copied().unwrap_or(0);
            let pending_approvals = counts.pending.get(&id).copied().unwrap_or(0);
            agents.push(AgentInfo {
                id,
                agent: h.agent.into(),
                project: h.project.into(),
                tmux_session: spec.tmux_session,
                state,
                unread_mail,
                pending_approvals,
                is_manager: h.is_manager,
            });
        }

        // Managers first, then workers; deterministic within each.
        agents.sort_by(|a, b| match (b.is_manager, a.is_manager) {
            (x, y) if x == y => a.id.cmp(&b.id),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Equal,
        });

        let mut channels = Vec::new();
        for project in &compose.projects {
            for ch in &project.channels {
                channels.push(ChannelInfo {
                    id: format!("{}:{}", project.project.id, ch.name),
                    name: ch.name.clone(),
                    project_id: project.project.id.clone(),
                });
            }
        }
        // Stable order for the picker — operators see the same
        // sequence on every open.
        channels.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Self {
            root: compose.root,
            team_name,
            agents,
            channels,
        })
    }
}

#[derive(Debug, Default)]
struct MailboxCounts {
    unread: HashMap<String, u32>,
    pending: HashMap<String, u32>,
}

/// Single sweep of the mailbox to populate per-agent counters. Read
/// errors degrade silently to zeroes — a missing or unreadable DB
/// is just "no team running yet" from the UI's perspective, not a
/// fatal launch error.
fn mailbox_counts(mailbox: &Path) -> Result<MailboxCounts> {
    if !mailbox.is_file() {
        return Ok(MailboxCounts::default());
    }
    let conn = Connection::open(mailbox)?;
    let mut counts = MailboxCounts::default();

    // Unread mail per recipient agent (channels excluded — channel
    // messages ack independently per subscriber and would require a
    // join we don't need in PR-UI-2).
    //
    // INVARIANT: every `messages.recipient` value falls into exactly
    // one of three prefix classes — `<project>:<agent>` (DM, no
    // scheme prefix; the channel-or-user split here relies on that
    // absence), `channel:<channel_id>`, or `user:<handle>`. The two
    // `NOT LIKE` clauses below treat anything outside the channel /
    // user prefixes as a per-agent DM. If a fourth prefix class
    // ever lands, every site that splits recipients (here,
    // `mailbox::BrokerMailboxSource::*` queries, and the tail.rs
    // follow loop) needs to learn it.
    let mut stmt = conn.prepare(
        "SELECT recipient, COUNT(*) FROM messages
         WHERE acked_at IS NULL
           AND recipient NOT LIKE 'channel:%'
           AND recipient NOT LIKE 'user:%'
         GROUP BY recipient",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows.flatten() {
        counts.unread.insert(row.0, row.1.max(0) as u32);
    }

    // Pending approvals per requesting agent.
    let mut stmt = conn.prepare(
        "SELECT project_id || ':' || agent_id, COUNT(*) FROM approvals
         WHERE status = 'pending'
         GROUP BY project_id, agent_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows.flatten() {
        counts.pending.insert(row.0, row.1.max(0) as u32);
    }

    Ok(counts)
}

/// Single-cell glyph for an agent's primary state — derived from the
/// triplet (`state`, `pending_approvals`, `unread_mail`) in priority
/// order: pending approval beats unread mail beats process state.
/// Plain ASCII fallback when the caller signals a monochrome /
/// no-symbol terminal.
pub fn state_glyph(info: &AgentInfo, fallback_ascii: bool) -> &'static str {
    match info.state {
        AgentState::Stopped => {
            if fallback_ascii {
                "x"
            } else {
                "✕"
            }
        }
        AgentState::Unknown => "?",
        AgentState::Running => {
            if info.pending_approvals > 0 {
                "!"
            } else if info.unread_mail > 0 {
                if fallback_ascii {
                    "@"
                } else {
                    "✉"
                }
            } else if fallback_ascii {
                "*"
            } else {
                "●"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(state: AgentState, unread: u32, pending: u32) -> AgentInfo {
        AgentInfo {
            id: "p:a".into(),
            agent: "a".into(),
            project: "p".into(),
            tmux_session: "t-p-a".into(),
            state,
            unread_mail: unread,
            pending_approvals: pending,
            is_manager: false,
        }
    }

    #[test]
    fn state_glyph_priorities_pending_then_unread_then_running() {
        assert_eq!(state_glyph(&info(AgentState::Running, 0, 0), false), "●");
        assert_eq!(state_glyph(&info(AgentState::Running, 3, 0), false), "✉");
        assert_eq!(state_glyph(&info(AgentState::Running, 3, 1), false), "!");
    }

    #[test]
    fn state_glyph_stopped_and_unknown() {
        assert_eq!(state_glyph(&info(AgentState::Stopped, 0, 0), false), "✕");
        assert_eq!(state_glyph(&info(AgentState::Unknown, 0, 0), false), "?");
    }

    #[test]
    fn state_glyph_ascii_fallback() {
        assert_eq!(state_glyph(&info(AgentState::Running, 0, 0), true), "*");
        assert_eq!(state_glyph(&info(AgentState::Running, 5, 0), true), "@");
        assert_eq!(state_glyph(&info(AgentState::Stopped, 0, 0), true), "x");
        // `!` and `?` are unchanged across the fallback boundary.
        assert_eq!(state_glyph(&info(AgentState::Running, 0, 1), true), "!");
        assert_eq!(state_glyph(&info(AgentState::Unknown, 0, 0), true), "?");
    }
}
