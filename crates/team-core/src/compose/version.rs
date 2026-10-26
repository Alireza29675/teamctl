use super::*;

/// T-265 PR-a: compose schema version. Stored as a semver string;
/// validate-time check (`validate::validate`) enforces the semver
/// shape via the `semver` crate.
///
/// **Custom Deserialize accepts two shapes:**
///
/// - YAML string (e.g. `version: "2.0.0"`) — taken verbatim; semver
///   shape is checked later at validate time, NOT here, so the
///   deserializer's job stays narrow (parse, not validate).
/// - YAML integer literal `2` only (the one legacy value that ever
///   shipped in any in-tree compose) → coerced to `SchemaVersion`
///   carrying `"2.0.0"` AND flagged `from_legacy_int = true`. The
///   load orchestration in [`Compose::load`] reads that flag to
///   decide whether to auto-rewrite the on-disk file so the
///   integer self-heals to the semver shape (owner-ratified tg
///   2989 + tg 3440, "option 1 + variant A").
///
/// Anything else — `version: 1`, `version: 3`, `version: true`,
/// `version: [1,2,3]` — fails to deserialize with a message that
/// names the constraint: only `"X.Y.Z"` or the legacy `2`.
///
/// `from_legacy_int` is `#[serde(skip)]` so it never round-trips
/// through serialize; it's a deserialize-side signal only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion {
    pub value: String,
    #[serde(skip)]
    pub from_legacy_int: bool,
}

impl SchemaVersion {
    // T-265: current compose schema version; bump on any schema-affecting type change.
    // 2.0.1: doc-only — McpServer.env description now names the codex
    // `${VAR}` interpolation gap (see McpEnvInterpolationUnsupported).
    pub const CURRENT: &str = "3.0.0";

    /// Construct directly from a semver-shaped string; for fixtures
    /// and tests + the in-memory legacy coercion.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            from_legacy_int: false,
        }
    }
}

// T-265: SchemaVersion has a custom Deserialize, so schemars can't
// introspect its shape. Present it to JSON Schema consumers as the
// thing a fresh compose file actually carries — a plain semver string.
impl JsonSchema for SchemaVersion {
    fn schema_name() -> String {
        "SchemaVersion".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        gen.subschema_for::<String>()
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = SchemaVersion;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a semver string like \"2.0.0\" (legacy integer `2` also accepted \
                     and auto-rewritten to \"2.0.0\" on next save)",
                )
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<SchemaVersion, E> {
                Ok(SchemaVersion {
                    value: s.to_string(),
                    from_legacy_int: false,
                })
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<SchemaVersion, E> {
                Ok(SchemaVersion {
                    value: s,
                    from_legacy_int: false,
                })
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<SchemaVersion, E> {
                if n == 2 {
                    Ok(SchemaVersion {
                        value: "2.0.0".to_string(),
                        from_legacy_int: true,
                    })
                } else {
                    Err(E::custom(format!(
                        "compose schema version must be a semver string like \"2.0.0\"; \
                         got integer {n} — only legacy `2` is auto-coerced"
                    )))
                }
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<SchemaVersion, E> {
                if n == 2 {
                    Ok(SchemaVersion {
                        value: "2.0.0".to_string(),
                        from_legacy_int: true,
                    })
                } else {
                    Err(E::custom(format!(
                        "compose schema version must be a semver string like \"2.0.0\"; \
                         got integer {n} — only legacy `2` is auto-coerced"
                    )))
                }
            }
        }
        d.deserialize_any(V)
    }
}
