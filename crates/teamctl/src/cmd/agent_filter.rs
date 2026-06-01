//! T-305: resolve the optional per-agent selector on `teamctl up |
//! down | reload <project> [...]` to a concrete set of agent names.
//!
//! Two operator-facing forms (owner-ratified, see #305):
//!
//! ```text
//! teamctl down <project> capture creator   -> AgentSelector::Only
//! teamctl down <project> --except gateway  -> AgentSelector::Except
//! ```
//!
//! With neither, the selector is `All` and the action keeps today's
//! "every agent in the resolved scope" contract untouched. The
//! positional list and `--except` are mutually exclusive (clap
//! enforces it); both are only valid alongside a project.

use std::collections::BTreeSet;

use anyhow::{anyhow, Result};
use team_core::compose::Compose;

/// Per-agent scope parsed from the CLI. `All` means no agent-level
/// filter — the no-arg / `<project>`-only contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSelector {
    All,
    Only(Vec<String>),
    Except(Vec<String>),
}

impl AgentSelector {
    /// Build from the two clap fields. clap already guarantees they are
    /// mutually exclusive and only present alongside a project, so the
    /// both-set case is unreachable in practice; if it ever slips
    /// through, the positional list wins rather than panicking.
    pub fn from_args(agents: Vec<String>, except: Vec<String>) -> Self {
        if !agents.is_empty() {
            AgentSelector::Only(agents)
        } else if !except.is_empty() {
            AgentSelector::Except(except)
        } else {
            AgentSelector::All
        }
    }

    /// True when the selector names specific agents (`Only`/`Except`) —
    /// i.e. the per-agent scope is active. `reload` keys its
    /// force-restart behavior off this.
    pub fn is_scoped(&self) -> bool {
        !matches!(self, AgentSelector::All)
    }
}

/// Resolve the selector against the agents that actually exist in
/// `project_id`.
///
/// - `Ok(None)` for `All` — the caller imposes no agent-level filter
///   and keeps the existing project-wide behavior verbatim.
/// - `Ok(Some(set))` for `Only`/`Except` — the concrete agent names to
///   act on.
///
/// Errors (listing every valid agent in the project) when a named
/// agent — in *either* form — is not part of that project. Validating
/// `--except` names too means a typo'd exclusion fails loudly instead
/// of silently acting on every agent.
pub fn resolve(
    compose: &Compose,
    project_id: &str,
    sel: &AgentSelector,
) -> Result<Option<BTreeSet<String>>> {
    let present: BTreeSet<String> = compose
        .agents()
        .filter(|h| h.project == project_id)
        .map(|h| h.agent.to_string())
        .collect();

    let named = match sel {
        AgentSelector::All => return Ok(None),
        AgentSelector::Only(v) | AgentSelector::Except(v) => v,
    };

    let unknown: Vec<&str> = named
        .iter()
        .filter(|a| !present.contains(a.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        let known = present.iter().cloned().collect::<Vec<_>>().join(", ");
        let known = if known.is_empty() {
            "(none)".to_string()
        } else {
            known
        };
        return Err(anyhow!(
            "unknown agent(s) in project `{project_id}`: {} — known: {known}",
            unknown.join(", ")
        ));
    }

    let targets: BTreeSet<String> = match sel {
        AgentSelector::Only(v) => v.iter().cloned().collect(),
        AgentSelector::Except(v) => {
            let ex: BTreeSet<&str> = v.iter().map(String::as_str).collect();
            present
                .iter()
                .filter(|a| !ex.contains(a.as_str()))
                .cloned()
                .collect()
        }
        AgentSelector::All => unreachable!("handled by the early return above"),
    };

    Ok(Some(targets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use team_core::compose::{
        Agent, Broker, Budget, Compose, Global, Hitl, Project, ProjectMeta, RateLimits,
        SupervisorCfg,
    };

    fn agent() -> Agent {
        Agent {
            runtime: "claude-code".into(),
            model: None,
            role_prompt: None,
            permission_mode: None,
            autonomy: "low_risk_only".into(),
            can_dm: vec![],
            can_broadcast: vec![],
            reports_to: None,
            on_rate_limit: None,
            effort: None,
            interfaces: None,
            display_name: None,
            hooks: vec![],
            mcps: Default::default(),
            subagents: vec![],
        }
    }

    /// One project, `agent_names` split as managers (first) + workers
    /// (rest) — enough surface for the selector; the manager/worker
    /// distinction is irrelevant to `resolve`.
    fn compose_with(project_id: &str, agent_names: &[&str]) -> Compose {
        let mut managers = BTreeMap::new();
        let mut workers = BTreeMap::new();
        for (i, n) in agent_names.iter().enumerate() {
            if i == 0 {
                managers.insert((*n).to_string(), agent());
            } else {
                workers.insert((*n).to_string(), agent());
            }
        }
        Compose {
            root: PathBuf::from("/tmp/fake"),
            global: Global {
                version: team_core::compose::SchemaVersion::new("2.0.0"),
                broker: Broker::default(),
                supervisor: SupervisorCfg::default(),
                budget: Budget::default(),
                hitl: Hitl::default(),
                rate_limits: RateLimits::default(),
                interfaces: vec![],
                projects: vec![],
                attachments: Default::default(),
            },
            projects: vec![Project {
                version: 1,
                project: ProjectMeta {
                    id: project_id.into(),
                    name: project_id.into(),
                    cwd: PathBuf::from("."),
                },
                channels: vec![],
                managers,
                workers,
                interfaces: None,
            }],
        }
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn from_args_maps_to_variants() {
        assert_eq!(AgentSelector::from_args(vec![], vec![]), AgentSelector::All);
        assert_eq!(
            AgentSelector::from_args(vec!["a".into()], vec![]),
            AgentSelector::Only(vec!["a".into()])
        );
        assert_eq!(
            AgentSelector::from_args(vec![], vec!["b".into()]),
            AgentSelector::Except(vec!["b".into()])
        );
        // Defensive both-set: positional list wins (clap prevents this).
        assert_eq!(
            AgentSelector::from_args(vec!["a".into()], vec!["b".into()]),
            AgentSelector::Only(vec!["a".into()])
        );
    }

    #[test]
    fn is_scoped_only_for_named_forms() {
        assert!(!AgentSelector::All.is_scoped());
        assert!(AgentSelector::Only(vec!["x".into()]).is_scoped());
        assert!(AgentSelector::Except(vec!["x".into()]).is_scoped());
    }

    #[test]
    fn all_imposes_no_filter() {
        let c = compose_with("p", &["mgr", "dev", "qa"]);
        assert_eq!(resolve(&c, "p", &AgentSelector::All).unwrap(), None);
    }

    #[test]
    fn only_returns_exactly_the_named_agents() {
        let c = compose_with("p", &["mgr", "dev", "qa"]);
        let got = resolve(
            &c,
            "p",
            &AgentSelector::Only(vec!["dev".into(), "qa".into()]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(got, set(&["dev", "qa"]));
    }

    #[test]
    fn except_returns_the_complement() {
        let c = compose_with("p", &["mgr", "dev", "qa"]);
        let got = resolve(&c, "p", &AgentSelector::Except(vec!["mgr".into()]))
            .unwrap()
            .unwrap();
        assert_eq!(got, set(&["dev", "qa"]));
    }

    #[test]
    fn except_every_agent_yields_empty_set() {
        // Excepting everyone is a valid (if pointless) op — it resolves
        // to "act on nothing", not an error. The callers print their
        // existing "no agents" line when the target set is empty.
        let c = compose_with("p", &["mgr", "dev"]);
        let got = resolve(
            &c,
            "p",
            &AgentSelector::Except(vec!["mgr".into(), "dev".into()]),
        )
        .unwrap()
        .unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn unknown_agent_in_only_errors_listing_valid_names() {
        let c = compose_with("p", &["mgr", "dev"]);
        let err = resolve(&c, "p", &AgentSelector::Only(vec!["ghost".into()]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost"), "names the bad input: {err}");
        assert!(err.contains("mgr"), "lists valid agents: {err}");
        assert!(err.contains("dev"), "lists valid agents: {err}");
    }

    #[test]
    fn unknown_agent_in_except_also_errors() {
        // A typo'd `--except` must fail loudly, not silently degrade to
        // "exclude nothing" and bounce every agent.
        let c = compose_with("p", &["mgr", "dev"]);
        let err = resolve(&c, "p", &AgentSelector::Except(vec!["gohst".into()]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("gohst"), "names the bad input: {err}");
        assert!(err.contains("mgr"), "lists valid agents: {err}");
    }

    #[test]
    fn only_dedups_repeated_names() {
        let c = compose_with("p", &["mgr", "dev"]);
        let got = resolve(
            &c,
            "p",
            &AgentSelector::Only(vec!["dev".into(), "dev".into()]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(got, set(&["dev"]));
    }
}
