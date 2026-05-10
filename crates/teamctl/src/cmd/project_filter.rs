//! T-133: resolve the optional `<project-name>` arg on `teamctl
//! up | down | reload` to a canonical `project.id`.
//!
//! Two match keys are accepted: the `project.id` field in the project
//! manifest, and the bare project filename (without `.yaml`). When both
//! resolve to *different* projects, the explicit `project.id` match
//! wins; the filename fallback is convenience for operators who think
//! in terms of `.team/projects/<name>.yaml`.

use std::path::Path;

use anyhow::{anyhow, Result};
use team_core::compose::Compose;

/// Resolve `name` to a canonical `project.id` against `compose`.
///
/// Returns `Err` listing every available `project.id` when no match is
/// found, so the operator gets a copy-pasteable hint instead of a bare
/// "unknown project" message.
pub fn resolve(compose: &Compose, name: &str) -> Result<String> {
    let mut by_id: Option<&str> = None;
    let mut by_file: Option<&str> = None;

    for (idx, p) in compose.projects.iter().enumerate() {
        if p.project.id == name {
            by_id = Some(p.project.id.as_str());
        }
        if let Some(r) = compose.global.projects.get(idx) {
            if Path::new(&r.file)
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| stem == name)
            {
                by_file = Some(p.project.id.as_str());
            }
        }
    }

    if let Some(id) = by_id.or(by_file) {
        return Ok(id.to_string());
    }

    let available = compose
        .projects
        .iter()
        .map(|p| p.project.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(anyhow!("unknown project `{name}` — known: {available}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use team_core::compose::{
        Broker, Budget, Compose, Global, Hitl, Project, ProjectMeta, ProjectRef, RateLimits,
        SupervisorCfg,
    };

    fn project(id: &str, name: &str) -> Project {
        Project {
            version: 1,
            project: ProjectMeta {
                id: id.into(),
                name: name.into(),
                cwd: PathBuf::from("."),
            },
            channels: vec![],
            managers: Default::default(),
            workers: Default::default(),
        }
    }

    fn compose_with(refs: Vec<(&str, &str, &str)>) -> Compose {
        // refs = [(filename, id, name)]
        Compose {
            root: PathBuf::from("/tmp/fake"),
            global: Global {
                version: 1,
                broker: Broker::default(),
                supervisor: SupervisorCfg::default(),
                budget: Budget::default(),
                hitl: Hitl::default(),
                rate_limits: RateLimits::default(),
                interfaces: vec![],
                projects: refs
                    .iter()
                    .map(|(f, _, _)| ProjectRef {
                        file: PathBuf::from(*f),
                    })
                    .collect(),
            },
            projects: refs.iter().map(|(_, id, n)| project(id, n)).collect(),
        }
    }

    #[test]
    fn resolve_matches_project_id_exactly() {
        let compose = compose_with(vec![("projects/teamctl.yaml", "teamctl", "Teamctl")]);
        assert_eq!(resolve(&compose, "teamctl").unwrap(), "teamctl");
    }

    #[test]
    fn resolve_matches_filename_stem_when_id_does_not_match() {
        // Operator thinks in terms of the file path; project.id is a
        // longer canonical form. Filename fallback exists for exactly
        // this case.
        let compose = compose_with(vec![("projects/dev.yaml", "dev-team", "Dev")]);
        assert_eq!(resolve(&compose, "dev").unwrap(), "dev-team");
    }

    #[test]
    fn resolve_prefers_project_id_when_id_and_file_resolve_to_different_projects() {
        // Edge case from the ticket: filename of project A matches the
        // id of project B. The explicit project.id match wins so a
        // freshly-renamed `id:` field is the source of truth, not the
        // legacy filename.
        let compose = compose_with(vec![
            ("projects/foo.yaml", "real-foo", "Foo"),
            ("projects/bar.yaml", "foo", "Bar"),
        ]);
        assert_eq!(resolve(&compose, "foo").unwrap(), "foo");
    }

    #[test]
    fn resolve_errors_with_available_list_on_miss() {
        let compose = compose_with(vec![
            ("projects/teamctl.yaml", "teamctl", "Teamctl"),
            ("projects/ops.yaml", "ops", "Ops"),
        ]);
        let err = resolve(&compose, "nope").unwrap_err().to_string();
        assert!(
            err.contains("nope"),
            "error names the rejected input: {err}"
        );
        assert!(err.contains("teamctl"), "error lists known projects: {err}");
        assert!(err.contains("ops"), "error lists known projects: {err}");
    }

    #[test]
    fn resolve_filename_match_returns_canonical_id_not_stem() {
        // The caller cares about the canonical project.id (used for
        // filtering agents downstream), not the filename it matched on.
        let compose = compose_with(vec![("projects/dev.yaml", "dev-team", "Dev")]);
        assert_eq!(resolve(&compose, "dev").unwrap(), "dev-team");
    }

    #[test]
    fn resolve_handles_empty_compose_with_clear_error() {
        // Operator running `teamctl up nonexistent` against a stripped
        // compose: no projects to suggest, but the message still names
        // what was rejected.
        let compose = compose_with(vec![]);
        let err = resolve(&compose, "nope").unwrap_err().to_string();
        assert!(err.contains("nope"), "error names the input: {err}");
    }
}
