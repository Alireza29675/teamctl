//! T-118: deterministic Claude Code session id derivation.
//!
//! Every claude-code agent spawn passes `--session-id <uuid>` so the
//! conversation persists across `teamctl down`/`up` cycles, crash
//! recovery, and host reboots. The id is a UUIDv5 derived from a
//! baked-in namespace plus the canonical agent string
//! `teamctl:<project>:<agent>`.
//!
//! Self-healing by construction: if the session-file at that UUID is
//! deleted (manual cleanup, claude session-dir reset, etc.), the next
//! spawn passes the same UUID and claude creates a fresh session at
//! it. No operator action required.
//!
//! Stable across rename of *external* things (claude version, host
//! machine, tmux session) but evicts cleanly when the project or
//! agent name changes — that's the right semantics: a renamed agent
//! is a new agent.

use uuid::Uuid;

/// Frozen UUIDv5 namespace for teamctl session-id derivation.
/// Generated once on 2026-05-10 and pinned forever — changing this
/// would invalidate every previously-resumed session across every
/// dogfooding installation. Treat as a constant.
///
/// Provenance: `uuid::Uuid::new_v4()` on 2026-05-10 in this repo's
/// development environment, pasted as a literal.
pub const TEAMCTL_SESSION_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6d, 0xd6, 0xc8, 0xa3, 0x44, 0xb6, 0x4a, 0x18, 0x9b, 0x05, 0x91, 0xc1, 0xe2, 0x57, 0xfb, 0x3d,
]);

/// Compose the canonical name string for a given project/agent pair.
/// Used as the UUIDv5 input AND surfaced verbatim to claude via the
/// `-n <name>` flag so the operator sees the agent identity in
/// claude's session picker / prompt box.
pub fn session_name(project: &str, agent: &str) -> String {
    format!("teamctl:{project}:{agent}")
}

/// Derive the deterministic Claude Code session id for a given
/// project/agent pair. Same inputs always yield the same UUID;
/// different agent or project yields a different UUID.
pub fn derive_session_id(project: &str, agent: &str) -> Uuid {
    Uuid::new_v5(
        &TEAMCTL_SESSION_NAMESPACE,
        session_name(project, agent).as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_yield_same_uuid() {
        // Determinism is the core invariant — every claude-code spawn
        // for `(hello, mgr)` must hit the same on-disk session file.
        let a = derive_session_id("hello", "mgr");
        let b = derive_session_id("hello", "mgr");
        assert_eq!(a, b);
    }

    #[test]
    fn different_agents_yield_different_uuids() {
        // Agents within the same project must not collide on
        // session-id; if they did, two agents would share a single
        // claude conversation and stomp each other's context.
        let mgr = derive_session_id("hello", "mgr");
        let dev = derive_session_id("hello", "dev");
        assert_ne!(mgr, dev);
    }

    #[test]
    fn different_projects_yield_different_uuids() {
        // Cross-project isolation: same agent name in two different
        // projects must resolve to two different sessions.
        let a = derive_session_id("alpha", "mgr");
        let b = derive_session_id("beta", "mgr");
        assert_ne!(a, b);
    }

    #[test]
    fn namespace_constant_is_stable() {
        // Pin the namespace bytes so an accidental edit (anyone
        // tempted to "regenerate the constant for cleanliness")
        // surfaces here instead of silently invalidating every
        // existing session across every install.
        assert_eq!(
            TEAMCTL_SESSION_NAMESPACE.to_string(),
            "6dd6c8a3-44b6-4a18-9b05-91c1e257fb3d"
        );
    }

    #[test]
    fn derived_uuid_is_v5_and_uses_namespace() {
        // Cross-check against an independent re-computation so a
        // refactor of `derive_session_id` that accidentally changes
        // the namespace or the input shape can't pass.
        let derived = derive_session_id("hello", "mgr");
        let expected = Uuid::new_v5(&TEAMCTL_SESSION_NAMESPACE, b"teamctl:hello:mgr");
        assert_eq!(derived, expected);
        assert_eq!(derived.get_version_num(), 5);
    }

    #[test]
    fn session_name_format_is_canonical() {
        // The `-n <name>` value the operator sees in claude's session
        // picker should mirror the UUID input shape so a session-id
        // collision (would be astronomical, but) is debuggable from
        // the human-readable name alone.
        assert_eq!(session_name("hello", "mgr"), "teamctl:hello:mgr");
    }
}
