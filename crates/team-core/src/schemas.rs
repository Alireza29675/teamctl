//! Compose JSON Schema: derive, bundle, and drift gate.
//!
//! `current_schema_json` is the schemars-derived JSON Schema for the
//! whole compose surface — the global `team-compose.yaml` tree
//! ([`Global`](crate::compose::Global)) AND the per-project file tree
//! ([`Project`](crate::compose::Project)), aggregated via
//! [`ComposeSchema`] (see below for why). A byte-identical copy lives
//! under `schemas/compose-<version>.json` and ships in the binary via
//! `include_str!`; the in-module drift test re-derives the schema and
//! asserts it matches the committed golden, so any schema-affecting
//! change to a compose type is caught in CI.

use crate::compose::{Global, Project, SchemaVersion};

/// T-265: schema-generation aggregate. NOT a real YAML file and never
/// deserialized — it exists only so `schema_for!` reaches BOTH the global
/// `team-compose.yaml` tree (`Global`) AND the per-project file tree
/// (`Project`, which `Global` only references by path via `ProjectRef`).
/// Without it the drift gate would miss every per-project type — notably
/// `Agent`, the most-churned schema surface — so a field added to `Agent`
/// could ship without a `version:` bump. The fields are read only by the
/// derived `JsonSchema` impl (which uses their *types*), hence
/// `allow(dead_code)`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct ComposeSchema {
    global: Global,
    project: Project,
}

/// The schemars-derived compose JSON Schema, pretty-printed with a
/// trailing newline so the committed golden is a stable, diffable file
/// (and matches what an editor / `git` expects at end-of-file).
pub fn current_schema_json() -> String {
    let mut s = serde_json::to_string_pretty(&schemars::schema_for!(ComposeSchema))
        .expect("schema serializes");
    s.push('\n');
    s
}

// T-265: bundled golden schemas keyed by schema version, following the
// `runtimes.rs` include_str! pattern. One entry per shipped schema
// version; `SchemaVersion::CURRENT` always has a row here.
// NOTE: `include_str!` needs a literal path, so the `2.0.1` below can't
// be `SchemaVersion::CURRENT`. Keep this filename in sync with CURRENT on
// every schema bump (add a new row; don't replace the old one — historical
// schemas stay bundled for the migration skill). The drift gate fails
// loudly if they desync, so a forgotten edit can't ship silently.
const EMBEDDED: &[(&str, &str)] = &[
    (
        SchemaVersion::CURRENT,
        include_str!("../schemas/compose-2.0.1.json"),
    ),
    ("2.0.0", include_str!("../schemas/compose-2.0.0.json")),
];

/// The bundled JSON Schema for a given schema version, or `None` when no
/// golden ships for it.
pub fn bundled(version: &str) -> Option<&'static str> {
    EMBEDDED
        .iter()
        .find(|(v, _)| *v == version)
        .map(|(_, schema)| *schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-265 drift gate: the re-derived schema must match the committed
    /// golden byte-for-byte. This is the gate that runs in CI via
    /// `cargo test --all`.
    #[test]
    fn schema_matches_committed_golden() {
        let derived = current_schema_json();
        let committed = bundled(SchemaVersion::CURRENT).expect("current golden is bundled");
        assert_eq!(
            derived,
            committed,
            "compose schema drifted from schemas/compose-{current}.json. \
             If this is an intentional schema change: bump SchemaVersion::CURRENT \
             (MAJOR=remove/rename/semantics, MINOR=new optional field, PATCH=doc-only), \
             then commit the regenerated schemas/compose-<new>.json. \
             If unintentional, revert the type change.",
            current = SchemaVersion::CURRENT,
        );
    }

    #[test]
    fn bundled_lookup_resolves_current_and_misses_unknown() {
        assert!(bundled(SchemaVersion::CURRENT).is_some());
        assert!(bundled("0.0.0").is_none());
    }

    /// Guards the ComposeSchema aggregate: the schema MUST cover the
    /// per-project tree, not just the global one. `Agent` is reachable
    /// only via `Project` (not `Global`, which references projects by
    /// path), so its presence proves the aggregate didn't regress to a
    /// `Global`-only root — which would silently stop the drift gate from
    /// catching `Agent` field changes.
    #[test]
    fn schema_covers_per_project_agent_tree() {
        let schema: serde_json::Value =
            serde_json::from_str(&current_schema_json()).expect("schema is valid json");
        assert!(
            schema["definitions"]["Agent"].is_object(),
            "compose schema must define the per-project `Agent` type; did ComposeSchema regress to Global-only?"
        );
    }

    /// T-265 regen helper (not a gate): rewrites the committed golden from
    /// the current types. Run after an intentional schema change +
    /// `SchemaVersion::CURRENT` bump:
    /// `cargo test -p team-core regen_golden -- --ignored`
    /// then commit the updated `schemas/compose-<CURRENT>.json`.
    #[test]
    #[ignore = "writes the source tree; run manually to regenerate the golden"]
    fn regen_golden() {
        let path = format!(
            "{}/schemas/compose-{}.json",
            env!("CARGO_MANIFEST_DIR"),
            SchemaVersion::CURRENT
        );
        std::fs::write(&path, current_schema_json()).expect("write golden");
    }
}
