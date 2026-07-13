//! Front-end-agnostic reporting-structure shape for a team.
//!
//! Computes the "You → managers → workers" reporting tree as structured
//! data — no ANSI, no ratatui — so both the CLI `init` preview and the
//! TUI detail panel can render it from a single source of truth. The
//! grouping and ordering mirror `teamctl init`'s team-structure preview
//! exactly (managers id-sorted via the `BTreeMap` key order, then orphan
//! workers at the top level, workers nested under the manager they
//! `reports_to`); the only thing dropped here is the color.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::compose;

/// One row of a team's reporting tree — depth-indented, front-end-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeRow {
    /// 0 = "You" root, 1 = manager / top-level orphan worker,
    /// 2 = worker under a manager.
    pub depth: u8,
    pub kind: ShapeKind,
    /// `display_name` when set, else the agent id.
    pub label: String,
    /// E.g. `"Claude Code · Opus 4.8 · 8×a 0×s 0×h 0×m"` (empty for Root).
    pub descriptor: String,
    /// Last sibling at this depth — for tree connectors.
    pub is_last: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Root,
    Manager,
    Worker,
}

/// Version of the JSON contract exchanged between `teamctl init` and the
/// standalone `teamctl-ui` picker.
pub const PICKER_PROTOCOL_VERSION: u32 = 1;

/// Catalog handed from `teamctl` (which owns and parses the templates) to
/// `teamctl-ui` (which only renders and selects from them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerCatalog {
    pub version: u32,
    pub entries: Vec<PickerCatalogEntry>,
}

/// One template the picker can render and return to its caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerCatalogEntry {
    /// Stable selection token understood by `teamctl init`.
    pub key: String,
    /// Operator-facing template name.
    pub name: String,
    /// Operator-facing summary. May be empty while catalogs are migrated to
    /// explicit curated descriptions.
    pub description: String,
    /// Precomputed reporting shape; the picker does no compose parsing.
    pub rows: Vec<ShapeRow>,
    pub counts: PreviewCounts,
}

/// Capability totals shown above a template's reporting tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewCounts {
    pub agents: usize,
    pub subagents: usize,
    pub skills: usize,
    pub hooks: usize,
    pub mcps: usize,
}

/// Machine-readable choice emitted by the standalone picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PickerResponse {
    Create { key: String },
    Customize { key: String },
    CoDesign,
}

impl PickerCatalog {
    /// Reject catalogs that would make selection ambiguous or render counts
    /// known to be inconsistent. Unknown JSON fields remain accepted by
    /// serde so newer producers can add fields without breaking this reader.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PICKER_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported picker protocol version {} (expected {PICKER_PROTOCOL_VERSION})",
                self.version
            ));
        }
        if self.entries.is_empty() {
            return Err("picker catalog must contain at least one entry".to_string());
        }

        let mut keys = BTreeSet::new();
        for entry in &self.entries {
            let key = entry.key.trim();
            if key.is_empty() {
                return Err("picker catalog entry key must not be blank".to_string());
            }
            if key == "guided" {
                return Err("picker catalog entry key `guided` is reserved".to_string());
            }
            if !keys.insert(key) {
                return Err(format!("duplicate picker catalog entry key `{key}`"));
            }
            if entry.name.trim().is_empty() {
                return Err(format!(
                    "picker catalog entry `{key}` name must not be blank"
                ));
            }

            let row_agents = entry
                .rows
                .iter()
                .filter(|row| row.kind != ShapeKind::Root)
                .count();
            if entry.counts.agents != row_agents {
                return Err(format!(
                    "picker catalog entry `{key}` reports {} agents but has {row_agents} non-root rows",
                    entry.counts.agents
                ));
            }
        }

        Ok(())
    }
}

/// Walk the given projects into one "You → managers → workers" tree.
///
/// One shared Root row, then per project: managers (id-sorted via the
/// `BTreeMap` key order) each followed by the workers that `reports_to`
/// them (id-sorted); workers whose `reports_to` isn't a manager in their
/// project hang at the top level (depth 1) after the managers. Mirrors
/// `init.rs::team_structure_lines`'s grouping/ordering exactly, minus
/// color.
pub fn team_shape(projects: &[&compose::Project]) -> Vec<ShapeRow> {
    let mut out = Vec::new();

    // One shared "You" root — the operator — emitted once at the top, so
    // the whole tree reads as a reporting hierarchy (managers report to
    // you; workers to their manager). init.rs emits this per project
    // because today's templates each ship a single agent-bearing
    // project; here we hoist it to a single root that spans every
    // project's top-level group.
    out.push(ShapeRow {
        depth: 0,
        kind: ShapeKind::Root,
        label: "You".to_string(),
        descriptor: String::new(),
        is_last: true,
    });

    for project in projects {
        if project.managers.is_empty() && project.workers.is_empty() {
            continue;
        }

        // Group workers under the manager they report to; any worker
        // whose `reports_to` isn't a manager in this project hangs at the
        // top level. BTreeMap iteration keeps each group id-sorted.
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
        // The two kinds are siblings at depth 1 within this project's span.
        let top: Vec<(&String, ShapeKind)> = project
            .managers
            .keys()
            .map(|id| (id, ShapeKind::Manager))
            .chain(orphans.iter().map(|id| (*id, ShapeKind::Worker)))
            .collect();

        let last_top = top.len().saturating_sub(1);
        for (i, (id, kind)) in top.iter().enumerate() {
            let agent = project
                .managers
                .get(*id)
                .or_else(|| project.workers.get(*id));
            let label = agent.map_or_else(|| (*id).to_string(), |a| label_for(id, a));
            let descriptor = agent.map(agent_descriptor).unwrap_or_default();
            out.push(ShapeRow {
                depth: 1,
                kind: *kind,
                label,
                descriptor,
                is_last: i == last_top,
            });

            // Workers reporting to this manager, nested one level in.
            let kids = children.get(id.as_str()).cloned().unwrap_or_default();
            let last_kid = kids.len().saturating_sub(1);
            for (j, wid) in kids.iter().enumerate() {
                let w = project.workers.get(wid.as_str());
                let label = w.map_or_else(|| (*wid).to_string(), |a| label_for(wid, a));
                let descriptor = w.map(agent_descriptor).unwrap_or_default();
                out.push(ShapeRow {
                    depth: 2,
                    kind: ShapeKind::Worker,
                    label,
                    descriptor,
                    is_last: j == last_kid,
                });
            }
        }
    }

    out
}

/// One-line descriptor for an agent: runtime label, model label (only
/// when pinned), then `N×a N×s N×h N×m` counts (subagents/skills/hooks/
/// mcps). Identical output to `init.rs::agent_descriptor`.
pub fn agent_descriptor(agent: &compose::Agent) -> String {
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

/// Roster label for an agent: its `display_name` when set, else the id.
fn label_for(id: &str, agent: &compose::Agent) -> String {
    agent.display_name.clone().unwrap_or_else(|| id.to_string())
}

/// Human-friendly runtime name; unknown runtimes show their raw id.
fn runtime_label(runtime: &str) -> String {
    match runtime {
        "claude-code" => "Claude Code".to_string(),
        other => other.to_string(),
    }
}

/// Human-friendly model name for the known Claude ids; anything else
/// shows the raw model string the operator pinned.
fn model_label(model: &str) -> String {
    match model {
        "claude-opus-4-8" => "Opus 4.8".to_string(),
        "claude-sonnet-4-6" => "Sonnet 4.6".to_string(),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => "Haiku 4.5".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(yaml: &str) -> compose::Project {
        serde_yaml::from_str(yaml).expect("project fixture parses")
    }

    #[test]
    fn single_manager_with_two_workers_orders_and_nests() {
        // Manager `lead`; workers `dev1` + `dev2` both report to it. The
        // root is emitted once; the manager sits at depth 1 (last sibling),
        // its two workers nest at depth 2, id-sorted with the last flagged.
        let p = project(
            "\
version: 1
project:
  id: demo
  name: Demo
  cwd: .
managers:
  lead:
    model: claude-opus-4-8
workers:
  dev2:
    reports_to: lead
  dev1:
    reports_to: lead
",
        );
        let rows = team_shape(&[&p]);
        assert_eq!(
            rows,
            vec![
                ShapeRow {
                    depth: 0,
                    kind: ShapeKind::Root,
                    label: "You".into(),
                    descriptor: String::new(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "lead".into(),
                    descriptor: "Claude Code · Opus 4.8 · 0×a 0×s 0×h 0×m".into(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 2,
                    kind: ShapeKind::Worker,
                    label: "dev1".into(),
                    descriptor: "Claude Code · 0×a 0×s 0×h 0×m".into(),
                    is_last: false,
                },
                ShapeRow {
                    depth: 2,
                    kind: ShapeKind::Worker,
                    label: "dev2".into(),
                    descriptor: "Claude Code · 0×a 0×s 0×h 0×m".into(),
                    is_last: true,
                },
            ]
        );
    }

    #[test]
    fn orphan_worker_hangs_at_top_level_after_managers() {
        // `solo` reports to `ghost`, which is not a manager in this
        // project, so it hangs at depth 1 after the (single) manager.
        // Managers come first, then orphans — and the orphan is the last
        // top-level sibling.
        let p = project(
            "\
version: 1
project:
  id: demo
  name: Demo
  cwd: .
managers:
  lead: {}
workers:
  solo:
    reports_to: ghost
",
        );
        let rows = team_shape(&[&p]);
        let top: Vec<_> = rows
            .iter()
            .filter(|r| r.depth == 1)
            .map(|r| (r.kind, r.label.as_str(), r.is_last))
            .collect();
        assert_eq!(
            top,
            vec![
                (ShapeKind::Manager, "lead", false),
                (ShapeKind::Worker, "solo", true),
            ]
        );
    }

    #[test]
    fn two_managers_sort_by_id() {
        // BTreeMap key order = id-sorted: `alpha` before `beta`, regardless
        // of YAML declaration order. The last manager is flagged `is_last`.
        let p = project(
            "\
version: 1
project:
  id: demo
  name: Demo
  cwd: .
managers:
  beta: {}
  alpha: {}
",
        );
        let rows = team_shape(&[&p]);
        let managers: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == ShapeKind::Manager)
            .map(|r| (r.label.as_str(), r.is_last))
            .collect();
        assert_eq!(managers, vec![("alpha", false), ("beta", true)]);
    }

    #[test]
    fn label_prefers_display_name_then_falls_back_to_id() {
        // `lead` has a display_name; `dev` does not. Label uses the
        // display_name when present, else the agent id.
        let p = project(
            "\
version: 1
project:
  id: demo
  name: Demo
  cwd: .
managers:
  lead:
    display_name: The Lead
workers:
  dev:
    reports_to: lead
",
        );
        let rows = team_shape(&[&p]);
        let labels: Vec<_> = rows
            .iter()
            .filter(|r| r.kind != ShapeKind::Root)
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["The Lead", "dev"]);
    }

    #[test]
    fn descriptor_omits_model_when_not_pinned() {
        // No `model:` → the model segment is dropped entirely; the
        // descriptor is just runtime + counts.
        let a: compose::Agent = serde_yaml::from_str("runtime: claude-code\n").unwrap();
        assert_eq!(agent_descriptor(&a), "Claude Code · 0×a 0×s 0×h 0×m");
    }

    #[test]
    fn descriptor_includes_model_when_pinned() {
        let a: compose::Agent =
            serde_yaml::from_str("runtime: claude-code\nmodel: claude-sonnet-4-6\n").unwrap();
        assert_eq!(
            agent_descriptor(&a),
            "Claude Code · Sonnet 4.6 · 0×a 0×s 0×h 0×m"
        );
    }

    #[test]
    fn descriptor_counts_subagents_skills_hooks_mcps() {
        // Two subagents, one skill, one hook, one mcp → `2×a 1×s 1×h 1×m`.
        let a: compose::Agent = serde_yaml::from_str(
            "\
runtime: claude-code
model: claude-opus-4-8
subagents:
  - subagents/reviewer.md
  - subagents/planner.md
skills:
  - skills/research
hooks:
  - event: PreToolUse
    command: hooks/guard.sh
mcps:
  github:
    command: npx
    args:
      - -y
      - github-mcp
",
        )
        .unwrap();
        assert_eq!(
            agent_descriptor(&a),
            "Claude Code · Opus 4.8 · 2×a 1×s 1×h 1×m"
        );
    }

    #[test]
    fn unknown_runtime_and_model_pass_through_raw() {
        let a: compose::Agent =
            serde_yaml::from_str("runtime: codex\nmodel: gpt-5-codex\n").unwrap();
        assert_eq!(
            agent_descriptor(&a),
            "codex · gpt-5-codex · 0×a 0×s 0×h 0×m"
        );
    }

    #[test]
    fn all_known_model_labels_render() {
        assert_eq!(model_label("claude-opus-4-8"), "Opus 4.8");
        assert_eq!(model_label("claude-sonnet-4-6"), "Sonnet 4.6");
        assert_eq!(model_label("claude-haiku-4-5"), "Haiku 4.5");
        assert_eq!(model_label("claude-haiku-4-5-20251001"), "Haiku 4.5");
    }

    #[test]
    fn empty_project_yields_only_the_shared_root() {
        let p = project(
            "\
version: 1
project:
  id: empty
  name: Empty
  cwd: .
",
        );
        let rows = team_shape(&[&p]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, ShapeKind::Root);
        assert_eq!(rows[0].label, "You");
        assert!(rows[0].descriptor.is_empty());
    }

    #[test]
    fn multi_project_shares_one_root_with_per_project_sibling_groups() {
        // Two agent-bearing projects share a single "You" root. Each
        // project's top-level managers form their own sibling group, so
        // each project's last manager is flagged `is_last` independently.
        let a = project(
            "\
version: 1
project:
  id: a
  name: A
  cwd: .
managers:
  am: {}
",
        );
        let b = project(
            "\
version: 1
project:
  id: b
  name: B
  cwd: .
managers:
  bm: {}
",
        );
        let rows = team_shape(&[&a, &b]);
        let roots = rows.iter().filter(|r| r.kind == ShapeKind::Root).count();
        assert_eq!(roots, 1, "only one shared root across projects");
        let managers: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == ShapeKind::Manager)
            .map(|r| (r.label.as_str(), r.is_last))
            .collect();
        // Each project's single manager is the last sibling in its own group.
        assert_eq!(managers, vec![("am", true), ("bm", true)]);
    }

    fn picker_entry(key: &str) -> PickerCatalogEntry {
        PickerCatalogEntry {
            key: key.to_string(),
            name: "Essentials".to_string(),
            // Blank is intentionally valid until every source has curated
            // catalog metadata.
            description: String::new(),
            rows: vec![
                ShapeRow {
                    depth: 0,
                    kind: ShapeKind::Root,
                    label: "You".to_string(),
                    descriptor: String::new(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "Lead".to_string(),
                    descriptor: "Claude Code · Opus 4.8".to_string(),
                    is_last: true,
                },
            ],
            counts: PreviewCounts {
                agents: 1,
                subagents: 2,
                skills: 3,
                hooks: 4,
                mcps: 5,
            },
        }
    }

    #[test]
    fn picker_catalog_json_round_trips_with_snake_case_shape_kinds() {
        let catalog = PickerCatalog {
            version: PICKER_PROTOCOL_VERSION,
            entries: vec![picker_entry("essentials")],
        };

        let json = serde_json::to_value(&catalog).unwrap();
        assert_eq!(json["version"], PICKER_PROTOCOL_VERSION);
        assert_eq!(json["entries"][0]["rows"][0]["kind"], "root");
        assert_eq!(json["entries"][0]["rows"][1]["kind"], "manager");
        assert_eq!(json["entries"][0]["counts"]["agents"], 1);
        assert_eq!(
            serde_json::from_value::<PickerCatalog>(json).unwrap(),
            catalog
        );
    }

    #[test]
    fn picker_responses_use_the_tagged_action_wire_shape() {
        let cases = [
            (
                PickerResponse::Create {
                    key: "essentials".to_string(),
                },
                serde_json::json!({"action": "create", "key": "essentials"}),
            ),
            (
                PickerResponse::Customize {
                    key: "essentials".to_string(),
                },
                serde_json::json!({"action": "customize", "key": "essentials"}),
            ),
            (
                PickerResponse::CoDesign,
                serde_json::json!({"action": "co_design"}),
            ),
        ];

        for (response, wire) in cases {
            assert_eq!(serde_json::to_value(&response).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<PickerResponse>(wire).unwrap(),
                response
            );
        }
    }

    #[test]
    fn picker_wire_ignores_unknown_fields_for_forward_compatibility() {
        let wire = serde_json::json!({
            "version": PICKER_PROTOCOL_VERSION,
            "future_catalog_field": true,
            "entries": [{
                "key": "essentials",
                "name": "Essentials",
                "description": "",
                "future_entry_field": [1, 2, 3],
                "rows": [{
                    "depth": 0,
                    "kind": "root",
                    "label": "You",
                    "descriptor": "",
                    "is_last": true,
                    "future_row_field": "ignored"
                }],
                "counts": {
                    "agents": 0,
                    "subagents": 0,
                    "skills": 0,
                    "hooks": 0,
                    "mcps": 0,
                    "future_count_field": 99
                }
            }]
        });

        let catalog: PickerCatalog = serde_json::from_value(wire).unwrap();
        assert!(catalog.validate().is_ok());

        let response: PickerResponse = serde_json::from_value(serde_json::json!({
            "action": "create",
            "key": "essentials",
            "future_response_field": true
        }))
        .unwrap();
        assert_eq!(
            response,
            PickerResponse::Create {
                key: "essentials".to_string()
            }
        );
    }

    #[test]
    fn picker_catalog_validation_accepts_blank_descriptions() {
        let catalog = PickerCatalog {
            version: PICKER_PROTOCOL_VERSION,
            entries: vec![picker_entry("essentials")],
        };

        assert_eq!(catalog.entries[0].description, "");
        assert_eq!(catalog.validate(), Ok(()));
    }

    #[test]
    fn picker_catalog_validation_rejects_invalid_wire_data() {
        let mut catalog = PickerCatalog {
            version: PICKER_PROTOCOL_VERSION + 1,
            entries: vec![picker_entry("essentials")],
        };
        assert!(catalog.validate().unwrap_err().contains("version"));

        catalog.version = PICKER_PROTOCOL_VERSION;
        catalog.entries.clear();
        assert!(catalog.validate().unwrap_err().contains("at least one"));

        catalog.entries = vec![picker_entry("  ")];
        assert!(catalog
            .validate()
            .unwrap_err()
            .contains("key must not be blank"));

        catalog.entries = vec![picker_entry(" guided ")];
        assert!(catalog.validate().unwrap_err().contains("reserved"));

        catalog.entries = vec![picker_entry("essentials"), picker_entry(" essentials ")];
        assert!(catalog.validate().unwrap_err().contains("duplicate"));

        catalog.entries = vec![picker_entry("essentials")];
        catalog.entries[0].name = "\t".to_string();
        assert!(catalog
            .validate()
            .unwrap_err()
            .contains("name must not be blank"));

        catalog.entries[0].name = "Essentials".to_string();
        catalog.entries[0].counts.agents = 2;
        assert!(catalog.validate().unwrap_err().contains("non-root rows"));
    }
}
