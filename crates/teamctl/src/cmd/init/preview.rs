use super::*;

/// The agent the operator most likely wants to attach to first: the lead
/// manager that workers report to (e.g. the Executor that fronts the team,
/// not a private back-channel manager like Compass), falling back to any
/// manager, then any agent at all. Returns the canonical `<project>:<agent>`
/// id `teamctl attach` expects, or `None` when the compose scaffolds no
/// agents (e.g. the `blank` template) — the caller then omits the
/// bot/attach hints in the `Next:` block.
pub(super) fn lead_attach_target(compose: &team_core::compose::Compose) -> Option<String> {
    let agents: Vec<_> = compose.agents().collect();
    // A manager counts as "lead" when a worker IN THE SAME PROJECT reports
    // to it. The project scope matters once a compose ships more than one
    // agent-bearing project — without it, a manager could falsely match a
    // same-named worker's `reports_to` in a different project. Today's
    // templates each ship a single agent-bearing project, so this is
    // behaviour-identical for them; it just keeps the function correct if
    // a multi-project template lands later.
    let reported_to = |mgr: &team_core::compose::AgentHandle| {
        agents
            .iter()
            .any(|w| w.project == mgr.project && w.spec.reports_to.as_deref() == Some(mgr.agent))
    };
    agents
        .iter()
        .find(|a| a.is_manager && reported_to(a))
        .or_else(|| agents.iter().find(|a| a.is_manager))
        .or_else(|| agents.first())
        .map(|h| h.id())
}

/// Build the team-structure preview from the in-memory template files
/// (not yet on disk at preview time). `team-compose.yaml` names the
/// project files; each `projects/<name>.yaml` deserializes into a
/// `Project`. Managers render at the top of each project; workers nest
/// under the manager they `reports_to` (so a reader sees the actual
/// reporting hierarchy, not a flat list). A worker that reports to no
/// listed manager hangs at the top level. Labels prefer `display_name`,
/// falling back to the id. The tree connectors stay grey while agent
/// names render in light pink (the `You` root in bold pink) — but only
/// when stderr is a terminal; piped/redirected output is plain so it
/// stays clean in logs and tests. Box-drawing matches the file tree.
///
/// Best-effort and non-fatal: a template with no parseable agents (e.g.
/// `blank`, or a compose we can't deserialize) yields an empty `Vec`,
/// and the caller simply skips the section. Init never fails on this.
pub(super) fn team_structure_lines(files: &[(String, &'static str)]) -> Vec<String> {
    // Gate raw ANSI on `crate::term::use_color()` — the NO_COLOR +
    // `is_terminal()` house convention warn.rs / bot.rs / release_notes.rs
    // follow, so `NO_COLOR=1` stays plain even on a TTY. The preview prints
    // to stderr, so that's the stream we test. (T-181)
    let color = crate::term::use_color(io::stderr().is_terminal());
    let paint = |code: &str, s: &str| -> String { paint_with(code, s, color) };
    const GREY: &str = "90"; // tree lines + the `(role)` suffix
    const PINK: &str = "38;5;218"; // agent names — light pink (256-colour)

    let by_path = |p: &str| files.iter().find(|(rel, _)| rel == p).map(|(_, c)| *c);

    // The project file relpaths to render, in compose-declared order.
    // If `team-compose.yaml` is missing or unparseable, fall back to any
    // `projects/*.yaml` the template ships so a hand-rolled tree still
    // previews.
    let project_paths: Vec<String> = by_path("team-compose.yaml")
        .and_then(|c| serde_yaml::from_str::<team_core::compose::Global>(c).ok())
        .map(|g| {
            g.projects
                .iter()
                .map(|p| p.file.to_string_lossy().into_owned())
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| {
            files
                .iter()
                .map(|(rel, _)| rel.clone())
                .filter(|rel| rel.starts_with("projects/") && rel.ends_with(".yaml"))
                .collect()
        });

    let mut out = Vec::new();
    for path in project_paths {
        let Some(project) = by_path(&path)
            .and_then(|c| serde_yaml::from_str::<team_core::compose::Project>(c).ok())
        else {
            continue;
        };
        if project.managers.is_empty() && project.workers.is_empty() {
            continue;
        }

        // Group workers under the manager they report to; any worker
        // whose `reports_to` isn't a manager in this project hangs at
        // the top level. BTreeMap iteration keeps each group id-sorted.
        let mut children: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
        let mut orphans: Vec<&String> = Vec::new();
        for (wid, w) in &project.workers {
            match w.reports_to.as_deref() {
                Some(m) if project.managers.contains_key(m) => {
                    children.entry(m).or_default().push(wid)
                }
                _ => orphans.push(wid),
            }
        }

        // Top level: every manager (id-sorted), then any orphan workers.
        let top: Vec<(&String, &str)> = project
            .managers
            .keys()
            .map(|id| (id, "manager"))
            .chain(orphans.iter().map(|id| (*id, "worker")))
            .collect();

        // Flush-left (no leading indent), whole tree in light pink with
        // grey connectors + a grey runtime/model/counts descriptor. The
        // root is "You" — the operator — so the tree reads as a reporting
        // hierarchy (managers report to you; workers to their manager).
        // (Current templates each ship one agent-bearing project, so this
        // emits a single "You"; a future multi-agent-project template
        // would want one shared "You" — flagged for the hardening pass.)
        out.push(paint(&format!("1;{PINK}"), "You"));

        let row = |indent: &str, connector: &str, label: &str, descriptor: &str| {
            format!(
                "{}{}{}  {}",
                indent,
                paint(GREY, connector),
                paint(PINK, label),
                paint(GREY, descriptor),
            )
        };

        let last_top = top.len().saturating_sub(1);
        for (i, (id, _role)) in top.iter().enumerate() {
            let agent = project
                .managers
                .get(*id)
                .or_else(|| project.workers.get(*id));
            let label = agent.map_or_else(|| (*id).to_string(), |a| label_for(id, a));
            let descriptor = agent.map(agent_descriptor).unwrap_or_default();
            let top_last = i == last_top;
            out.push(row(
                "",
                if top_last { "└── " } else { "├── " },
                &label,
                &descriptor,
            ));

            // Workers reporting to this manager, nested one level in. The
            // continuation column is a grey `│` unless this manager is the
            // last top-level entry, in which case it's blank.
            let kids = children.get(id.as_str()).cloned().unwrap_or_default();
            let cont = if top_last {
                "    ".to_string()
            } else {
                format!("{}   ", paint(GREY, "│"))
            };
            let last_kid = kids.len().saturating_sub(1);
            for (j, wid) in kids.iter().enumerate() {
                let w = project.workers.get(wid.as_str());
                let label = w.map_or_else(|| (*wid).to_string(), |a| label_for(wid, a));
                let descriptor = w.map(agent_descriptor).unwrap_or_default();
                let conn = if j == last_kid {
                    "└── "
                } else {
                    "├── "
                };
                out.push(row(&cont, conn, &label, &descriptor));
            }
        }
    }
    out
}

/// SGR-paint `s` with `code`, but only when `color` is set. Split out of
/// `team_structure_lines` (like warn.rs / bot.rs) so the NO_COLOR / TTY
/// gate is unit-testable without a real terminal. (T-181)
pub(super) fn paint_with(code: &str, s: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Roster label for an agent: its `display_name` when set, else the id —
/// the same fallback the teamctl-ui roster uses.
pub(super) fn label_for(id: &str, agent: &team_core::compose::Agent) -> String {
    agent.display_name.clone().unwrap_or_else(|| id.to_string())
}

/// One-line descriptor for an agent in the team tree: runtime, model (when
/// pinned), and counts of the per-agent settings — `N×a` sub-agents, `N×s`
/// skills, `N×h` hooks, `N×m` mcps. E.g. `Claude Code · 8×a 0×s 0×h 0×m`.
pub(super) fn agent_descriptor(agent: &team_core::compose::Agent) -> String {
    let mut parts = vec![runtime_label(&agent.runtime)];
    if let Some(model) = &agent.model {
        parts.push(model_label(model));
    }
    parts.push(format!(
        "{}×a {}×s {}×h {}×m",
        agent.subagents.len(),
        agent.skills.len(),
        agent.hooks.len(),
        agent.mcps.len(),
    ));
    parts.join(" · ")
}

/// Human-friendly runtime name; unknown runtimes show their raw id.
pub(super) fn runtime_label(runtime: &str) -> String {
    match runtime {
        "claude-code" => "Claude Code".to_string(),
        other => other.to_string(),
    }
}

/// Human-friendly model name for the known Claude ids; anything else shows
/// the raw model string the operator pinned.
pub(super) fn model_label(model: &str) -> String {
    match model {
        "claude-opus-4-8" => "Opus 4.8".to_string(),
        "claude-sonnet-4-6" => "Sonnet 4.6".to_string(),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => "Haiku 4.5".to_string(),
        other => other.to_string(),
    }
}

/// A node in the scaffold file tree, used to render the `teamctl init`
/// confirmation as a `tree`-style hierarchy instead of a flat path list.
/// Children preserve first-seen (template) order; a node with children is
/// rendered as a directory (trailing `/`), a leaf as a file.
#[derive(Default)]
pub(super) struct TreeNode {
    pub(super) children: Vec<(String, TreeNode)>,
}

impl TreeNode {
    /// Insert a path's `/`-split components, creating intermediate nodes.
    pub(super) fn insert(&mut self, parts: &[&str]) {
        let Some((head, rest)) = parts.split_first() else {
            return;
        };
        let idx = match self.children.iter().position(|(n, _)| n == head) {
            Some(i) => i,
            None => {
                self.children
                    .push(((*head).to_string(), TreeNode::default()));
                self.children.len() - 1
            }
        };
        self.children[idx].1.insert(rest);
    }

    /// Emit each line via `out`, prefixing with box-drawing connectors.
    pub(super) fn render(&self, prefix: &str, out: &mut impl FnMut(String)) {
        let last_idx = self.children.len().saturating_sub(1);
        for (i, (name, child)) in self.children.iter().enumerate() {
            let last = i == last_idx;
            let connector = if last { "└── " } else { "├── " };
            let slash = if child.children.is_empty() { "" } else { "/" };
            out(format!("{prefix}{connector}{name}{slash}"));
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            child.render(&child_prefix, out);
        }
    }
}
