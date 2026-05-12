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
    /// T-160: optional human-friendly label from
    /// `team-compose.yaml`. When `Some`, the TUI renders this in place
    /// of `id` everywhere an agent label surfaces to the operator
    /// (roster, detail header, mailbox attribution, statusline,
    /// approvals, compose modal). When `None`, label falls back to
    /// `id`. The id stays canonical for routing/tmux/CLI.
    pub display_name: Option<String>,
    /// T-212: most recent rate-limit reset timestamp (unix epoch
    /// seconds) for this agent, sourced from the `rate_limits` table
    /// populated by `teamctl rl-watch`. `None` when rl-watch has
    /// never recorded a `resets_at` for this agent. The TUI status
    /// bar formats this against `now()` via
    /// [`format_rate_limit_window`] to render "5m 12s" / "1h 23m" —
    /// past timestamps render as `None` (no active limit).
    pub rate_limit_resets_at: Option<f64>,
    /// T-211: short agent name (the YAML key in the manager's project)
    /// this agent reports to. `None` for top-level agents (no parent)
    /// — they render at depth 0 in the Agents pane. When `Some`, the
    /// renderer nests this row under its manager with a tree glyph.
    /// Schema validation (`team-core/src/validate.rs`) guarantees the
    /// referenced name resolves to an existing agent.
    pub reports_to: Option<String>,
}

/// Return the operator-facing label for `agent_id`: the agent's
/// `display_name` when set, otherwise `agent_id` itself. Read-only
/// borrow into the snapshot — callers that need an owned `String`
/// can `.to_string()` at the use-site. Unknown ids fall through to
/// `agent_id` (the canonical id is always a valid label).
pub fn agent_label<'a>(team: &'a TeamSnapshot, agent_id: &'a str) -> &'a str {
    team.agents
        .iter()
        .find(|a| a.id == agent_id)
        .and_then(|a| a.display_name.as_deref())
        .unwrap_or(agent_id)
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
            let display_name = h.spec.display_name.clone();
            let reports_to = h.spec.reports_to.clone();
            let spec =
                AgentSpec::from_handle(h, &compose.root, &compose.global.supervisor.tmux_prefix);
            let state = supervisor.state(&spec).unwrap_or(AgentState::Unknown);
            let id = h.id();
            let unread_mail = counts.unread.get(&id).copied().unwrap_or(0);
            let pending_approvals = counts.pending.get(&id).copied().unwrap_or(0);
            let rate_limit_resets_at = counts.rate_limit.get(&id).copied();
            agents.push(AgentInfo {
                id,
                agent: h.agent.into(),
                project: h.project.into(),
                tmux_session: spec.tmux_session,
                state,
                unread_mail,
                pending_approvals,
                is_manager: h.is_manager,
                display_name,
                rate_limit_resets_at,
                reports_to,
            });
        }

        // Managers first, then workers; deterministic within each.
        agents.sort_by(|a, b| match (b.is_manager, a.is_manager) {
            (x, y) if x == y => a.id.cmp(&b.id),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Equal,
        });

        // T-211: reorder into tree-DFS so the roster reads top-down
        // (manager → that manager's reports → next manager → …).
        // Selection stays index-based + sticky-on-id (`replace_team`
        // hunts by id), so nav still walks the visible order without
        // any selection-state refactor. Teams with no `reports_to`
        // usage degenerate to the prior order — every agent stays at
        // depth 0 and the Vec is byte-identical to the post-sort
        // shape above.
        agents = into_tree_dfs_order(agents);

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

/// Per-row metadata the Agents pane renderer needs to draw the
/// `reports_to` tree (T-211). Computed by [`tree_row_meta`] over a
/// `Vec<AgentInfo>` that's already in tree-DFS order (i.e. produced
/// by [`into_tree_dfs_order`] during `TeamSnapshot::load`). Lives
/// next to the agents Vec rather than on `AgentInfo` itself because
/// it's purely view-layer state — the data struct stays clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRowMeta {
    /// Depth from the top of the tree. Top-level agents (no
    /// `reports_to`) are depth 0; their direct reports are depth 1.
    /// V1 schema is one-level (worker → manager); a defensive depth
    /// >= 2 case falls back to depth 1 in the renderer.
    pub depth: usize,
    /// True iff this row is the last child of its parent in render
    /// order (or, for depth 0, the last top-level agent). Drives the
    /// `└─` vs `├─` glyph choice in the renderer.
    pub is_last_sibling: bool,
}

/// Reorder `agents` into depth-first tree order: each top-level
/// (`reports_to == None`) agent is followed by its direct reports
/// in their pre-existing order, recursively. The input order
/// (managers-first sorted by id within each group, then workers)
/// is preserved at each tier — this only **interleaves** workers
/// under their manager rather than putting them all in a flat
/// post-managers block.
///
/// Teams with no `reports_to` usage are passed through unchanged.
/// Orphan rows (reports_to references a missing agent — should be
/// caught by validation but checked defensively) are appended at
/// the end.
pub fn into_tree_dfs_order(agents: Vec<AgentInfo>) -> Vec<AgentInfo> {
    if agents.iter().all(|a| a.reports_to.is_none()) {
        return agents; // Fast path: no tree, no reorder.
    }
    // Scope the parent lookup to (project, agent_name) since
    // `reports_to` resolves within the project per validate.rs:185.
    let name_to_index: HashMap<(&str, &str), usize> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| ((a.project.as_str(), a.agent.as_str()), i))
        .collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut top_level: Vec<usize> = Vec::new();
    for (i, a) in agents.iter().enumerate() {
        let parent_idx = a
            .reports_to
            .as_deref()
            .and_then(|p| name_to_index.get(&(a.project.as_str(), p)).copied());
        match parent_idx {
            Some(p) => children.entry(p).or_default().push(i),
            None => top_level.push(i),
        }
    }
    let mut emitted = vec![false; agents.len()];
    let mut order: Vec<usize> = Vec::with_capacity(agents.len());
    fn walk(
        i: usize,
        children: &HashMap<usize, Vec<usize>>,
        emitted: &mut [bool],
        order: &mut Vec<usize>,
    ) {
        if emitted[i] {
            return; // Defensive: also breaks any cycle past validation.
        }
        emitted[i] = true;
        order.push(i);
        if let Some(kids) = children.get(&i) {
            for &k in kids {
                walk(k, children, emitted, order);
            }
        }
    }
    for &i in &top_level {
        walk(i, &children, &mut emitted, &mut order);
    }
    // Defensive: schema validation rejects cycles + dangling parents,
    // but if anything slipped past we'd rather render it at the end
    // than drop it from the roster entirely.
    for (i, &was_emitted) in emitted.iter().enumerate() {
        if !was_emitted {
            order.push(i);
        }
    }
    let mut indexed: Vec<Option<AgentInfo>> = agents.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|i| indexed[i].take())
        .collect()
}

/// Compute per-row tree metadata for an `agents` slice that's already
/// in DFS order (post-[`into_tree_dfs_order`]). The renderer pairs
/// this 1:1 with the Vec to draw `├─` / `└─` glyphs.
pub fn tree_row_meta(agents: &[AgentInfo]) -> Vec<TreeRowMeta> {
    if agents.iter().all(|a| a.reports_to.is_none()) {
        // Fast path: every row is its own top-level entry. `is_last_sibling`
        // is true for the last top-level row only (drives no glyph at
        // depth 0 today, but keeps the contract honest for any future
        // depth-0 separator).
        let n = agents.len();
        return (0..n)
            .map(|i| TreeRowMeta {
                depth: 0,
                is_last_sibling: i + 1 == n,
            })
            .collect();
    }
    let name_to_index: HashMap<(&str, &str), usize> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| ((a.project.as_str(), a.agent.as_str()), i))
        .collect();
    // Resolved parent index per agent, or None for top-level / orphan.
    let parents: Vec<Option<usize>> = agents
        .iter()
        .map(|a| {
            a.reports_to
                .as_deref()
                .and_then(|p| name_to_index.get(&(a.project.as_str(), p)).copied())
        })
        .collect();
    // Depth: 0 for top-level / orphan, else parent's depth + 1.
    // V1 schema is one-level so a single forward pass suffices —
    // DFS order guarantees parents precede children.
    let mut depth = vec![0usize; agents.len()];
    for i in 0..agents.len() {
        if let Some(p) = parents[i] {
            depth[i] = depth[p] + 1;
        }
    }
    // is_last_sibling: per parent (or `None` bucket for top-level),
    // the highest-index row in that bucket is the last.
    let mut last_in_bucket: HashMap<Option<usize>, usize> = HashMap::new();
    for (i, p) in parents.iter().enumerate() {
        last_in_bucket
            .entry(*p)
            .and_modify(|stored| {
                if i > *stored {
                    *stored = i;
                }
            })
            .or_insert(i);
    }
    (0..agents.len())
        .map(|i| TreeRowMeta {
            depth: depth[i],
            is_last_sibling: last_in_bucket.get(&parents[i]).copied() == Some(i),
        })
        .collect()
}

#[derive(Debug, Default)]
struct MailboxCounts {
    unread: HashMap<String, u32>,
    pending: HashMap<String, u32>,
    /// T-212: per-agent latest `rate_limits.resets_at` (unix epoch
    /// seconds). Only the most recent rate-limit row per agent that
    /// has a non-null `resets_at` lands here — rows with no parsed
    /// reset time are still recorded by `rl-watch` for forensics
    /// but don't drive UI.
    rate_limit: HashMap<String, f64>,
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

    // T-212: latest rate-limit reset per agent. The `rate_limits`
    // table is populated by `teamctl rl-watch`. We pick the most
    // recent row per agent (by `id`, which is monotonically
    // increasing per the schema in team-core::mailbox), and only
    // when `resets_at` is non-null — null-resets-at rows are still
    // logged by rl-watch for debugging the parser but don't drive
    // UI state. Past-`resets_at` filtering happens at format time
    // in [`format_rate_limit_window`] so we don't need a `now()`
    // dependency in this query.
    //
    // Table missing (rl-watch never ran on this mailbox) →
    // `prepare()` errors and we degrade silently to empty map,
    // matching the surrounding "no data is fine" pattern.
    if let Ok(mut stmt) = conn.prepare(
        "SELECT agent_id, resets_at FROM rate_limits
         WHERE id IN (
             SELECT MAX(id) FROM rate_limits
             WHERE resets_at IS NOT NULL
             GROUP BY agent_id
         )",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))
        {
            for row in rows.flatten() {
                counts.rate_limit.insert(row.0, row.1);
            }
        }
    }

    Ok(counts)
}

/// T-212: format a rate-limit reset timestamp as a short label for
/// the status bar. Returns `None` when the limit is in the past, at
/// the current instant, or unset — the indicator hides in those
/// cases. For active limits, formats as `42s` (under a minute),
/// `5m 12s` (under an hour), or `1h 23m` (an hour or more).
/// Operator-facing string; not for parsing.
pub fn format_rate_limit_window(resets_at: Option<f64>, now_unix: f64) -> Option<String> {
    let resets_at = resets_at?;
    let remaining = resets_at - now_unix;
    if remaining <= 0.0 {
        return None;
    }
    let secs = remaining as u64;
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        Some(format!("{hours}h {mins}m"))
    } else if secs >= 60 {
        let mins = secs / 60;
        let s = secs % 60;
        Some(format!("{mins}m {s}s"))
    } else {
        Some(format!("{secs}s"))
    }
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
            display_name: None,
            rate_limit_resets_at: None,
            reports_to: None,
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

    // T-212: format_rate_limit_window covers the value-shape rules
    // the status-bar slot will render against. The SQL-extension
    // path is exercised by integration tests at the snapshot layer
    // (matching the existing untested-at-this-layer pattern for
    // unread/pending) — the formatter is the part with branchy
    // logic worth pinning.

    #[test]
    fn format_rate_limit_window_returns_none_when_unset() {
        assert_eq!(format_rate_limit_window(None, 1000.0), None);
    }

    #[test]
    fn format_rate_limit_window_returns_none_when_already_past() {
        assert_eq!(format_rate_limit_window(Some(500.0), 1000.0), None);
    }

    #[test]
    fn format_rate_limit_window_returns_none_at_exact_now() {
        assert_eq!(format_rate_limit_window(Some(1000.0), 1000.0), None);
    }

    #[test]
    fn format_rate_limit_window_under_minute_renders_seconds() {
        assert_eq!(
            format_rate_limit_window(Some(1042.0), 1000.0),
            Some("42s".into())
        );
        assert_eq!(
            format_rate_limit_window(Some(1059.0), 1000.0),
            Some("59s".into())
        );
    }

    #[test]
    fn format_rate_limit_window_under_hour_renders_minutes_and_seconds() {
        assert_eq!(
            format_rate_limit_window(Some(1060.0), 1000.0),
            Some("1m 0s".into())
        );
        assert_eq!(
            format_rate_limit_window(Some(1312.0), 1000.0),
            Some("5m 12s".into())
        );
    }

    #[test]
    fn format_rate_limit_window_at_or_over_hour_renders_hours_and_minutes() {
        assert_eq!(
            format_rate_limit_window(Some(4600.0), 1000.0),
            Some("1h 0m".into())
        );
        assert_eq!(
            format_rate_limit_window(Some(5980.0), 1000.0),
            Some("1h 23m".into())
        );
    }
}
