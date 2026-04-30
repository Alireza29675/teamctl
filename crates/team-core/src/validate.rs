//! Invariant checks for a loaded [`Compose`] tree.
//!
//! Errors are collected rather than returned on first failure so the CLI can
//! pretty-print the full list.

use std::collections::BTreeSet;

use thiserror::Error;

use crate::compose::{ChannelMembers, Compose};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("project `{0}`: duplicate agent id `{1}` in managers and workers")]
    DuplicateAgent(String, String),

    #[error(
        "project `{project}`: unknown agent `{agent}` referenced in channel `{channel}` members"
    )]
    ChannelUnknownMember {
        project: String,
        channel: String,
        agent: String,
    },

    #[error("project `{project}`: agent `{agent}` `can_dm` lists unknown agent `{target}`")]
    DmUnknownTarget {
        project: String,
        agent: String,
        target: String,
    },

    #[error(
        "project `{project}`: agent `{agent}` `can_broadcast` lists unknown channel `{channel}`"
    )]
    BroadcastUnknownChannel {
        project: String,
        agent: String,
        channel: String,
    },

    #[error(
        "project `{project}`: agent `{agent}` has `telegram_inbox: true` but is not a manager"
    )]
    TelegramInboxOnWorker { project: String, agent: String },

    #[error(
        "project `{project}`: agent `{agent}` has `reports_to_user: true` but is not a manager"
    )]
    ReportsToUserOnWorker { project: String, agent: String },

    #[error(
        "worker `{project}:{agent}` declares `reports_to: {target}` but no such manager exists"
    )]
    UnknownManager {
        project: String,
        agent: String,
        target: String,
    },

    #[error("broker type `{0}` not supported (known: sqlite)")]
    UnknownBroker(String),

    #[error("supervisor type `{0}` not supported (known: tmux, systemd, launchd)")]
    UnknownSupervisor(String),

    #[error("duplicate project id `{0}`")]
    DuplicateProject(String),

    #[error("project `{project}`: agent `{agent}` uses runtime `{runtime}`, which is not built in and not declared in `<root>/runtimes/{runtime}.yaml`")]
    UnknownRuntime {
        project: String,
        agent: String,
        runtime: String,
    },
}

pub fn validate(compose: &Compose) -> Vec<ValidationError> {
    let mut errs = Vec::new();

    // Known runtimes. Embedded defaults are always present, so the
    // validator can always enforce that every referenced runtime resolves
    // to a descriptor (built in or user-supplied override).
    let runtimes = crate::runtimes::load_all(&compose.root).unwrap_or_default();
    let check_runtime = !runtimes.is_empty();

    match compose.global.broker.r#type.as_str() {
        "sqlite" => {}
        other => errs.push(ValidationError::UnknownBroker(other.into())),
    }
    match compose.global.supervisor.r#type.as_str() {
        "tmux" | "systemd" | "launchd" => {}
        other => errs.push(ValidationError::UnknownSupervisor(other.into())),
    }

    let mut seen_projects = BTreeSet::new();
    for p in &compose.projects {
        if !seen_projects.insert(p.project.id.clone()) {
            errs.push(ValidationError::DuplicateProject(p.project.id.clone()));
        }

        let mgr_ids: BTreeSet<&str> = p.managers.keys().map(|s| s.as_str()).collect();
        let wrk_ids: BTreeSet<&str> = p.workers.keys().map(|s| s.as_str()).collect();
        for dup in mgr_ids.intersection(&wrk_ids) {
            errs.push(ValidationError::DuplicateAgent(
                p.project.id.clone(),
                (*dup).to_string(),
            ));
        }
        let all_agents: BTreeSet<&str> = mgr_ids.union(&wrk_ids).copied().collect();

        // Channel members reference known agents.
        let channel_names: BTreeSet<&str> = p.channels.iter().map(|c| c.name.as_str()).collect();
        for ch in &p.channels {
            if let ChannelMembers::Explicit(members) = &ch.members {
                for m in members {
                    if !all_agents.contains(m.as_str()) {
                        errs.push(ValidationError::ChannelUnknownMember {
                            project: p.project.id.clone(),
                            channel: ch.name.clone(),
                            agent: m.clone(),
                        });
                    }
                }
            }
        }

        // Per-agent checks.
        let check_agent = |errs: &mut Vec<ValidationError>,
                           id: &str,
                           a: &crate::compose::Agent,
                           is_manager: bool| {
            if a.telegram_inbox && !is_manager {
                errs.push(ValidationError::TelegramInboxOnWorker {
                    project: p.project.id.clone(),
                    agent: id.into(),
                });
            }
            if a.reports_to_user && !is_manager {
                errs.push(ValidationError::ReportsToUserOnWorker {
                    project: p.project.id.clone(),
                    agent: id.into(),
                });
            }
            for t in &a.can_dm {
                if !all_agents.contains(t.as_str()) {
                    errs.push(ValidationError::DmUnknownTarget {
                        project: p.project.id.clone(),
                        agent: id.into(),
                        target: t.clone(),
                    });
                }
            }
            for c in &a.can_broadcast {
                if !channel_names.contains(c.as_str()) {
                    errs.push(ValidationError::BroadcastUnknownChannel {
                        project: p.project.id.clone(),
                        agent: id.into(),
                        channel: c.clone(),
                    });
                }
            }
            if let Some(t) = &a.reports_to {
                if !mgr_ids.contains(t.as_str()) {
                    errs.push(ValidationError::UnknownManager {
                        project: p.project.id.clone(),
                        agent: id.into(),
                        target: t.clone(),
                    });
                }
            }
            if check_runtime && !runtimes.contains_key(a.runtime.as_str()) {
                errs.push(ValidationError::UnknownRuntime {
                    project: p.project.id.clone(),
                    agent: id.into(),
                    runtime: a.runtime.clone(),
                });
            }
        };

        for (id, a) in &p.managers {
            check_agent(&mut errs, id, a, true);
        }
        for (id, a) in &p.workers {
            check_agent(&mut errs, id, a, false);
        }
    }

    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn toy_compose(agent_dm_target: &str) -> Compose {
        let mut managers = BTreeMap::new();
        managers.insert(
            "mgr".into(),
            Agent {
                runtime: "claude-code".into(),
                model: Some("claude-opus-4-7".into()),
                role_prompt: None,
                permission_mode: None,
                telegram_inbox: true,
                reports_to_user: true,
                autonomy: "low_risk_only".into(),
                can_dm: vec![agent_dm_target.into()],
                can_broadcast: vec!["team".into()],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
            },
        );
        let mut workers = BTreeMap::new();
        workers.insert(
            "dev".into(),
            Agent {
                runtime: "claude-code".into(),
                model: None,
                role_prompt: None,
                permission_mode: None,
                telegram_inbox: false,
                reports_to_user: false,
                autonomy: "low_risk_only".into(),
                can_dm: vec!["mgr".into()],
                can_broadcast: vec!["team".into()],
                reports_to: Some("mgr".into()),
                on_rate_limit: None,
                effort: None,
            },
        );
        Compose {
            root: PathBuf::from("."),
            global: Global {
                version: 2,
                broker: Default::default(),
                supervisor: Default::default(),
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
            },
            projects: vec![Project {
                version: 2,
                project: ProjectMeta {
                    id: "hello".into(),
                    name: "Hello".into(),
                    cwd: PathBuf::from("."),
                },
                channels: vec![Channel {
                    name: "team".into(),
                    members: ChannelMembers::All("*".into()),
                }],
                managers,
                workers,
            }],
        }
    }

    #[test]
    fn clean_compose_validates() {
        let c = toy_compose("dev");
        assert_eq!(validate(&c), vec![]);
    }

    #[test]
    fn dm_to_unknown_agent_flags() {
        let c = toy_compose("ghost");
        let e = validate(&c);
        assert!(matches!(
            e.as_slice(),
            [ValidationError::DmUnknownTarget { .. }]
        ));
    }

    #[test]
    fn unknown_broker_flags() {
        let mut c = toy_compose("dev");
        c.global.broker.r#type = "redis".into();
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::UnknownBroker(_))));
    }
}
