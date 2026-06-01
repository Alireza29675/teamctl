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
        "project `{project}`: agent `{agent}` has an `interfaces.telegram` block but is not a manager"
    )]
    TelegramInboxOnWorker { project: String, agent: String },

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

    #[error(
        "project id `{0}` has disallowed characters; allowed: ASCII letters, digits, and `.` `_` `-` (no whitespace, shell metacharacters, or control chars; `:` is reserved as the project:agent separator)"
    )]
    InvalidProjectId(String),

    #[error(
        "project `{project}`: agent id `{agent}` has disallowed characters; allowed: ASCII letters, digits, and `.` `_` `-` (no whitespace, shell metacharacters, or control chars; `:` is reserved as the project:agent separator)"
    )]
    InvalidAgentId { project: String, agent: String },

    #[error("project `{project}`: agent `{agent}` uses runtime `{runtime}`, which is not built in and not declared in `<root>/runtimes/{runtime}.yaml`")]
    UnknownRuntime {
        project: String,
        agent: String,
        runtime: String,
    },

    #[error("supervisor.drain_timeout_secs={0} is unreasonable; expected 0..=600")]
    DrainTimeoutOutOfRange(u64),

    #[error(
        "compose schema `version: {got}` is not a valid semver string (expected e.g. `\"2.0.0\"`)"
    )]
    SchemaVersionInvalid { got: String },

    #[error(
        "project `{project}`: agent `{agent}` has a blank `role_prompt` (empty string or empty list)"
    )]
    BlankRolePrompt { project: String, agent: String },

    #[error("project `{project}`: agent `{agent}` has a blank `display_name`")]
    BlankDisplayName { project: String, agent: String },

    #[error("project `{project}`: agent `{agent}` `display_name` is {got} chars (max {max})")]
    DisplayNameTooLong {
        project: String,
        agent: String,
        got: usize,
        max: usize,
    },

    #[error(
        "project `{project}`: agent `{agent}` declares an MCP server named `team`, which is reserved for the built-in mailbox server"
    )]
    ReservedMcpServerName { project: String, agent: String },
}

/// T-160: max length for `display_name`. 64 is a sensible upper bound
/// matching ratatui column widths in the TUI roster pane; longer names
/// would force unsightly truncation downstream. Counted in Unicode
/// scalar values (`chars().count()`), not grapheme clusters — most
/// operator-typed labels are simple text where the two coincide, and
/// the saved dependency on `unicode-segmentation` is not worth the
/// fidelity gain for an at-most-64-cell rendering window.
pub const DISPLAY_NAME_MAX_CHARS: usize = 64;

/// T-310: charset rule for `project.id` and agent ids — both flow,
/// unquoted, into shell-bound strings (`{project}:{agent}` in
/// `supervisor::build_up_command`) and into tmux session names. A
/// conservative ASCII allowlist keeps every downstream consumer safe
/// at the boundary instead of forcing each call site to quote
/// defensively. `:` is intentionally NOT allowed — it's reserved as
/// the canonical `<project>:<agent>` join separator everywhere in
/// teamctl, and admitting it into an id would make `id()` parse
/// ambiguous.
///
/// Rejected by construction: any whitespace, any shell metacharacter
/// (`;`, `|`, `&`, `$`, backtick, `(`, `)`, `<`, `>`, `*`, `?`, `~`,
/// `!`, `#`, `'`, `"`, `\`), any control char, any non-ASCII (incl.
/// emoji), the empty string.
pub fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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
    if compose.global.supervisor.drain_timeout_secs > 600 {
        errs.push(ValidationError::DrainTimeoutOutOfRange(
            compose.global.supervisor.drain_timeout_secs,
        ));
    }

    // T-265 PR-a: compose schema version must be a valid semver
    // string. Deserialization already accepted only (a) a string
    // (passed verbatim into `SchemaVersion::value`) or (b) the
    // legacy integer `2` (coerced to `"2.0.0"`); this validate-time
    // check rejects in-string garbage like `"abc"` or `"2"` (the
    // bare `"2"` is NOT semver — it needs the `.0.0` suffix).
    // Delegated to the `semver` crate to get prerelease /
    // build-metadata edge cases right rather than hand-rolling.
    if semver::Version::parse(&compose.global.version.value).is_err() {
        errs.push(ValidationError::SchemaVersionInvalid {
            got: compose.global.version.value.clone(),
        });
    }

    let mut seen_projects = BTreeSet::new();
    for p in &compose.projects {
        if !seen_projects.insert(p.project.id.clone()) {
            errs.push(ValidationError::DuplicateProject(p.project.id.clone()));
        }
        // T-310: id charset gate. Both `project.id` and the agent-id
        // map keys flow unquoted into shell-bound strings + tmux
        // session names downstream; rejecting unsafe chars at the
        // boundary hardens every consumer at once.
        if !is_valid_id(&p.project.id) {
            errs.push(ValidationError::InvalidProjectId(p.project.id.clone()));
        }
        for id in p.managers.keys().chain(p.workers.keys()) {
            if !is_valid_id(id) {
                errs.push(ValidationError::InvalidAgentId {
                    project: p.project.id.clone(),
                    agent: id.clone(),
                });
            }
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
            if a.telegram().is_some() && !is_manager {
                errs.push(ValidationError::TelegramInboxOnWorker {
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
            if let Some(rp) = &a.role_prompt {
                if rp.is_blank() {
                    errs.push(ValidationError::BlankRolePrompt {
                        project: p.project.id.clone(),
                        agent: id.into(),
                    });
                }
            }
            if let Some(dn) = &a.display_name {
                // T-160: trim before checking so `display_name: "   "`
                // is rejected as blank rather than slipping through
                // (whitespace-only labels render as a void cell in the
                // TUI — same operator-confusion shape as empty).
                let trimmed_len = dn.trim().chars().count();
                if trimmed_len == 0 {
                    errs.push(ValidationError::BlankDisplayName {
                        project: p.project.id.clone(),
                        agent: id.into(),
                    });
                } else if dn.chars().count() > DISPLAY_NAME_MAX_CHARS {
                    errs.push(ValidationError::DisplayNameTooLong {
                        project: p.project.id.clone(),
                        agent: id.into(),
                        got: dn.chars().count(),
                        max: DISPLAY_NAME_MAX_CHARS,
                    });
                }
            }
            // #383 Phase 4: `team` is the built-in mailbox MCP server,
            // injected on every agent. A declared server of the same name
            // would shadow the bus, so reject it at validate (render also
            // skips it defensively).
            if a.mcps.contains_key("team") {
                errs.push(ValidationError::ReservedMcpServerName {
                    project: p.project.id.clone(),
                    agent: id.into(),
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
                model: Some("claude-opus-4-8".into()),
                role_prompt: None,
                permission_mode: None,
                autonomy: "low_risk_only".into(),
                can_dm: vec![agent_dm_target.into()],
                can_broadcast: vec!["team".into()],
                reports_to: None,
                on_rate_limit: None,
                effort: None,
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
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
                autonomy: "low_risk_only".into(),
                can_dm: vec!["mgr".into()],
                can_broadcast: vec!["team".into()],
                reports_to: Some("mgr".into()),
                on_rate_limit: None,
                effort: None,
                interfaces: None,
                display_name: None,
                hooks: vec![],
                mcps: Default::default(),
                subagents: vec![],
            },
        );
        Compose {
            root: PathBuf::from("."),
            global: Global {
                version: crate::compose::SchemaVersion::new("2.0.0"),
                broker: Default::default(),
                supervisor: Default::default(),
                budget: Default::default(),
                hitl: Default::default(),
                rate_limits: Default::default(),
                interfaces: vec![],
                projects: vec![],
                attachments: Default::default(),
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
                interfaces: None,
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

    #[test]
    fn drain_timeout_above_600s_flags() {
        let mut c = toy_compose("dev");
        c.global.supervisor.drain_timeout_secs = 86_400;
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::DrainTimeoutOutOfRange(86_400))));
    }

    #[test]
    fn drain_timeout_zero_is_valid() {
        let mut c = toy_compose("dev");
        c.global.supervisor.drain_timeout_secs = 0;
        assert!(!validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::DrainTimeoutOutOfRange(_))));
    }

    #[test]
    fn empty_role_prompt_list_flags() {
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(crate::compose::RolePrompt::Multiple(vec![]));
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankRolePrompt { .. })));
    }

    #[test]
    fn empty_role_prompt_string_flags() {
        // Pre-existing hole in the old `Option<PathBuf>` schema: an
        // empty string slipped through and rendered
        // `SYSTEM_PROMPT_PATH=<root>/`. Closed alongside the list
        // form's empty-list check.
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt =
            Some(crate::compose::RolePrompt::Single(PathBuf::from("")));
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankRolePrompt { .. })));
    }

    #[test]
    fn single_role_prompt_validates() {
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(
            crate::compose::RolePrompt::Single(PathBuf::from("roles/mgr.md")),
        );
        assert!(!validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankRolePrompt { .. })));
    }

    #[test]
    fn populated_role_prompt_list_validates() {
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().role_prompt = Some(
            crate::compose::RolePrompt::Multiple(vec![PathBuf::from("roles/mgr.md")]),
        );
        assert!(!validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankRolePrompt { .. })));
    }

    #[test]
    fn blank_display_name_flags() {
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().display_name = Some(String::new());
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankDisplayName { .. })));
    }

    #[test]
    fn declared_mcp_server_named_team_flags() {
        // #383 Phase 4: `team` is the reserved built-in mailbox server;
        // declaring one of the same name must be a validation error.
        let mut c = toy_compose("dev");
        let mut mcps = std::collections::BTreeMap::new();
        mcps.insert(
            "team".into(),
            crate::compose::McpServer {
                command: "evil".into(),
                args: vec![],
                env: Default::default(),
            },
        );
        c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::ReservedMcpServerName { .. })));
    }

    #[test]
    fn declared_mcp_server_with_normal_name_validates() {
        // A non-reserved server name must NOT trip the reserved check.
        let mut c = toy_compose("dev");
        let mut mcps = std::collections::BTreeMap::new();
        mcps.insert(
            "github".into(),
            crate::compose::McpServer {
                command: "npx".into(),
                args: vec![],
                env: Default::default(),
            },
        );
        c.projects[0].managers.get_mut("mgr").unwrap().mcps = mcps;
        assert!(!validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::ReservedMcpServerName { .. })));
    }

    #[test]
    fn display_name_at_max_length_validates() {
        let mut c = toy_compose("dev");
        let exactly_max = "x".repeat(DISPLAY_NAME_MAX_CHARS);
        c.projects[0].managers.get_mut("mgr").unwrap().display_name = Some(exactly_max);
        assert!(!validate(&c).iter().any(|e| matches!(
            e,
            ValidationError::BlankDisplayName { .. } | ValidationError::DisplayNameTooLong { .. }
        )));
    }

    #[test]
    fn display_name_above_max_length_flags() {
        let mut c = toy_compose("dev");
        let too_long = "x".repeat(DISPLAY_NAME_MAX_CHARS + 1);
        c.projects[0].managers.get_mut("mgr").unwrap().display_name = Some(too_long);
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::DisplayNameTooLong { .. })));
    }

    #[test]
    fn display_name_counts_chars_not_bytes() {
        // T-160: limit applies to Unicode-scalar-value (`chars()`)
        // count, not bytes. Each `🦀` is one char but four UTF-8
        // bytes; 64 crabs must still validate even though that's 256
        // bytes on disk.
        let mut c = toy_compose("dev");
        let sixty_four_crabs = "🦀".repeat(DISPLAY_NAME_MAX_CHARS);
        c.projects[0].managers.get_mut("mgr").unwrap().display_name = Some(sixty_four_crabs);
        assert!(!validate(&c).iter().any(|e| matches!(
            e,
            ValidationError::BlankDisplayName { .. } | ValidationError::DisplayNameTooLong { .. }
        )));
    }

    #[test]
    fn whitespace_only_display_name_flags_blank() {
        // T-160 qa follow-up: whitespace-only labels render as a void
        // cell in the TUI — reject under the same `BlankDisplayName`
        // error as empty strings so the operator gets a clear message.
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().display_name = Some("   ".into());
        assert!(validate(&c)
            .iter()
            .any(|e| matches!(e, ValidationError::BlankDisplayName { .. })));
    }

    #[test]
    fn populated_display_name_validates() {
        let mut c = toy_compose("dev");
        c.projects[0].managers.get_mut("mgr").unwrap().display_name =
            Some("Sage (Visionary)".into());
        assert!(!validate(&c).iter().any(|e| matches!(
            e,
            ValidationError::BlankDisplayName { .. } | ValidationError::DisplayNameTooLong { .. }
        )));
    }

    // ── T-310: id charset validation ────────────────────────────────────

    #[test]
    fn is_valid_id_accepts_existing_id_shapes() {
        // Pin the conformant shapes currently in use across dogfood,
        // cookbook, tests, and conventional fixtures — none of these
        // may regress.
        for ok in [
            "teamctl",
            "ops",
            "nico",
            "eng_lead",
            "pr-22-review",
            "blog-site",
            "my.team",
            "a1",
            "x-2.0",
            "a",
            "0",
            "A",
            "_",
            "-",
            ".",
        ] {
            assert!(is_valid_id(ok), "must accept conformant id `{ok}`");
        }
    }

    #[test]
    fn is_valid_id_rejects_shell_metacharacter_class() {
        // The qa PoC class on #310: any shell metacharacter in an id
        // would flow unquoted into `build_up_command`. Each of these
        // must be rejected at the boundary.
        for bad in [
            "evil; rm",
            "proj$(id)",
            "with space",
            "back`ticks`",
            "p|ipe",
            "p&amp",
            "p*g",
            "p?g",
            "p~e",
            "p!g",
            "p#g",
            "p'q",
            "p\"q",
            "p\\g",
            "p<g",
            "p>g",
            "p(g",
            "p)g",
            "p\tg",
            "p\ng",
        ] {
            assert!(!is_valid_id(bad), "must reject `{bad:?}`");
        }
    }

    #[test]
    fn is_valid_id_rejects_colon_as_reserved_separator() {
        // `:` is reserved for the canonical `<project>:<agent>` join;
        // admitting it into an id would make `AgentHandle::id()`
        // ambiguous *and* would still flow unquoted into shell-bound
        // strings.
        assert!(!is_valid_id("p:rj"));
        assert!(!is_valid_id(":"));
        assert!(!is_valid_id("a:"));
        assert!(!is_valid_id(":a"));
    }

    #[test]
    fn is_valid_id_rejects_empty_and_control_chars() {
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("\0"));
        assert!(!is_valid_id("p\x07q"));
    }

    #[test]
    fn is_valid_id_rejects_non_ascii() {
        // Emoji and Unicode letters are operationally bad even though
        // some are shell-inert; stay strictly ASCII per the issue.
        assert!(!is_valid_id("crab🦀"));
        assert!(!is_valid_id("café"));
    }

    #[test]
    fn clean_compose_passes_id_charset() {
        // Regression for conformant teams (acceptance criterion 5):
        // existing valid ids are unaffected.
        let c = toy_compose("dev");
        let errs = validate(&c);
        assert!(
            !errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidProjectId(_) | ValidationError::InvalidAgentId { .. }
            )),
            "clean compose unexpectedly flagged for id charset: {errs:?}",
        );
    }

    #[test]
    fn project_id_with_shell_metacharacters_flags() {
        // qa PoC class regression (acceptance criterion 4): a
        // `project.id` containing shell-metacharacter content is
        // rejected by `validate` *before* it could reach
        // `build_up_command`'s unquoted shell interpolation on
        // `teamctl up` / `reload` / `down`.
        let mut c = toy_compose("dev");
        c.projects[0].project.id = "evil; rm -rf ~".into();
        let errs = validate(&c);
        assert!(
            errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidProjectId(s) if s == "evil; rm -rf ~"
            )),
            "expected InvalidProjectId, got {errs:?}",
        );
    }

    #[test]
    fn manager_id_with_shell_metacharacters_flags() {
        // Same PoC class via the manager-key path — `compose.agents()`
        // yields these ids and the supervisor flows them unquoted into
        // `{project}:{agent}`.
        let mut c = toy_compose("dev");
        let bad = "$(id)";
        let mgr = c.projects[0].managers.remove("mgr").unwrap();
        c.projects[0].managers.insert(bad.into(), mgr);
        let errs = validate(&c);
        assert!(
            errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidAgentId { project, agent }
                    if project == "hello" && agent == bad
            )),
            "expected InvalidAgentId for manager, got {errs:?}",
        );
    }

    #[test]
    fn worker_id_with_shell_metacharacters_flags() {
        // Same PoC class via the worker-key path.
        let mut c = toy_compose("dev");
        let bad = "rogue|pipe";
        let wkr = c.projects[0].workers.remove("dev").unwrap();
        c.projects[0].workers.insert(bad.into(), wkr);
        // The dm target `dev` is also stale now — filter for the
        // charset error specifically.
        let errs = validate(&c);
        assert!(
            errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidAgentId { project, agent }
                    if project == "hello" && agent == bad
            )),
            "expected InvalidAgentId for worker, got {errs:?}",
        );
    }

    #[test]
    fn project_id_with_reserved_colon_flags() {
        // `:` is the canonical project:agent join — admitting it would
        // make routing parse ambiguous *and* still leak unquoted into
        // shell strings.
        let mut c = toy_compose("dev");
        c.projects[0].project.id = "foo:bar".into();
        let errs = validate(&c);
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::InvalidProjectId(s) if s == "foo:bar")),
            "expected InvalidProjectId on colon, got {errs:?}",
        );
    }

    // T-265 PR-a: schema version semver-shape check. Deserialize
    // accepts any string verbatim (narrow concern); this validate
    // step is what enforces the shape.

    #[test]
    fn valid_semver_string_validates() {
        // Default fixture uses "2.0.0" — the canonical form.
        let c = toy_compose("dev");
        assert!(
            !validate(&c)
                .iter()
                .any(|e| matches!(e, ValidationError::SchemaVersionInvalid { .. })),
            "canonical version `2.0.0` must validate"
        );
    }

    #[test]
    fn malformed_semver_string_flags() {
        let mut c = toy_compose("dev");
        c.global.version = crate::compose::SchemaVersion::new("abc");
        let errs = validate(&c);
        assert!(
            errs.iter().any(|e| matches!(
                e,
                ValidationError::SchemaVersionInvalid { got } if got == "abc"
            )),
            "non-semver string must surface SchemaVersionInvalid; got {errs:?}"
        );
    }

    #[test]
    fn bare_two_string_flags_too() {
        // `"2"` is NOT semver (needs `.0.0`). Bare-2-string would
        // sneak past if our check were too loose; pin it explicitly.
        let mut c = toy_compose("dev");
        c.global.version = crate::compose::SchemaVersion::new("2");
        assert!(
            validate(&c).iter().any(|e| matches!(
                e,
                ValidationError::SchemaVersionInvalid { got } if got == "2"
            )),
            "bare-2-string must NOT pass the semver shape check"
        );
    }

    #[test]
    fn semver_with_prerelease_and_build_metadata_validates() {
        // Real-world semver supports `-pre` + `+build` suffixes.
        // Delegating to the `semver` crate gets these right;
        // pin the contract so a future "let's hand-roll a regex"
        // refactor surfaces here.
        for ok in [
            "1.0.0",
            "2.3.4",
            "2.0.0-alpha",
            "1.0.0+build.5",
            "2.0.0-rc.1+build.7",
        ] {
            let mut c = toy_compose("dev");
            c.global.version = crate::compose::SchemaVersion::new(ok);
            assert!(
                !validate(&c)
                    .iter()
                    .any(|e| matches!(e, ValidationError::SchemaVersionInvalid { .. })),
                "semver `{ok}` must validate"
            );
        }
    }
}
