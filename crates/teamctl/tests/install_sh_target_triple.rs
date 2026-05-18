//! Regression guard for #322.
//!
//! `tools/install.sh` downloads per-platform release tarballs named
//! `<bin>-<TARGET>.tar.xz`, where `TARGET` comes from a hardcoded
//! `OS-ARCH` → triple case block. cargo-dist only publishes assets for
//! the targets listed under `[workspace.metadata.dist]` in the root
//! `Cargo.toml`. If the installer asks for a triple cargo-dist does not
//! build, every fetch 404s and teamctl is uninstallable via the primary
//! `curl … | sh` path — exactly the #322 P0, caused by #294/#309
//! switching the Linux release artifacts to `-linux-musl` while the
//! installer still asked for `-linux-gnu`.
//!
//! Exercising the real download needs network + a published release, so
//! this is a static-content guard instead: it pins the invariant that
//! every triple the installer can resolve is a triple cargo-dist
//! actually publishes, and that the Linux triples are musl (the
//! released-artifact shape, locked so it cannot silently drift back).
//!
//! #324: the `parse_dist_targets` parser is shape-robust over both
//! multi-line and inline `targets = […]` arrays AND anchored to the
//! `[workspace.metadata.dist]` section so a sibling sub-table with its
//! own `targets = [` (e.g. `dependencies.apt`'s per-package targets)
//! cannot shadow the cargo-dist asset list.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `TARGET=<triple>` the installer's `OS-ARCH` case block can
/// assign. `$TARGET` *uses* (e.g. `$bin-$TARGET.tar.xz`) are not
/// assignments and don't match `TARGET=`.
fn installer_targets() -> BTreeSet<String> {
    read("tools/install.sh")
        .lines()
        .filter_map(|line| {
            let t = line.trim_start();
            if t.starts_with('#') {
                return None;
            }
            let after = t.split("TARGET=").nth(1)?;
            // First whitespace/`;`-delimited token after `TARGET=`.
            after
                .split([' ', '\t', ';'])
                .find(|s| !s.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// The triples cargo-dist publishes assets for: the `targets = [ … ]`
/// array in the root `Cargo.toml` `[workspace.metadata.dist]` table.
fn dist_targets() -> BTreeSet<String> {
    parse_dist_targets(&read("Cargo.toml"))
}

/// Section-anchored, shape-robust parse of `[workspace.metadata.dist]`'s
/// `targets = […]` array. Handles both the multi-line shape used today
/// and a future inline collapse. Anchored to the `[workspace.metadata.
/// dist]` section so a sibling sub-table (e.g. `dependencies.apt`'s
/// per-package `targets = […]`) cannot shadow the cargo-dist asset list.
fn parse_dist_targets(toml: &str) -> BTreeSet<String> {
    const SECTION: &str = "[workspace.metadata.dist]";
    const ARRAY: &str = "targets = [";

    let section_start = toml
        .find(SECTION)
        .unwrap_or_else(|| panic!("no `{SECTION}` table in Cargo.toml"));
    let after_header = &toml[section_start + SECTION.len()..];
    // Section body ends at the next table header (`\n[…]`). The dotted
    // sub-tables of `[workspace.metadata.dist]` (e.g.
    // `.dependencies.apt`) also start with `\n[` and are correctly
    // treated as outside this section's body.
    let body_end = after_header.find("\n[").unwrap_or(after_header.len());
    let section = &after_header[..body_end];

    let arr_start = section.find(ARRAY).unwrap_or_else(|| {
        panic!("no `{ARRAY}` inside `{SECTION}` — did the cargo-dist config move?")
    });
    let after_open = &section[arr_start + ARRAY.len()..];
    let arr_end = after_open
        .find(']')
        .unwrap_or_else(|| panic!("`{ARRAY}` has no closing `]` inside `{SECTION}`"));
    let body = &after_open[..arr_end];

    // The body is a comma-separated list of `"triple"` string literals,
    // possibly multi-line. Splitting on `"` and taking odd-indexed
    // segments yields the unquoted triples for either layout. (TOML
    // does not escape `"` inside double-quoted strings without `\\`,
    // and cargo-dist triples never contain quotes or backslashes.)
    body.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

#[test]
fn installer_only_requests_published_targets() {
    let installer = installer_targets();
    let dist = dist_targets();

    assert!(
        installer.len() >= 4,
        "expected >=4 TARGET= assignments in tools/install.sh, found {} \
         ({installer:?}) — did the platform case block move?",
        installer.len()
    );
    assert!(
        dist.len() >= 4,
        "expected >=4 cargo-dist targets in root Cargo.toml, found {} \
         ({dist:?}) — did the [workspace.metadata.dist] table move?",
        dist.len()
    );

    let unpublished: Vec<_> = installer.difference(&dist).collect();
    assert!(
        unpublished.is_empty(),
        "tools/install.sh resolves target(s) cargo-dist does NOT publish: \
         {unpublished:?}. Every <bin>-<TARGET>.tar.xz fetch for these 404s \
         and teamctl is uninstallable via curl|sh (#322). install.sh \
         targets={installer:?}, cargo-dist targets={dist:?}."
    );
}

#[test]
fn installer_linux_targets_are_musl() {
    for target in installer_targets() {
        if target.contains("-linux-") {
            assert!(
                target.ends_with("-linux-musl"),
                "tools/install.sh resolves Linux target `{target}` — the \
                 release artifacts are static-musl only since #294/#309, so \
                 a non-musl Linux triple here 404s every Linux install \
                 (#322). Use `*-unknown-linux-musl`."
            );
        }
    }
}

// ────────── parser-shape unit tests (#324) ──────────
//
// These exercise `parse_dist_targets` against synthetic Cargo.toml-shaped
// inputs covering both shapes the parser must handle (multi-line and
// inline) AND the section-anchoring that prevents a sibling sub-table's
// `targets = [` (e.g. `dependencies.apt`'s) from being mistakenly read as
// the cargo-dist asset list.

#[test]
fn parser_handles_multiline_array() {
    let toml = r#"
[workspace.metadata.dist]
cargo-dist-version = "0.25.1"
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
]
pr-run-mode = "plan"
"#;
    let got = parse_dist_targets(toml);
    assert_eq!(got.len(), 4, "parsed {got:?}");
    assert!(got.contains("x86_64-unknown-linux-musl"));
    assert!(got.contains("aarch64-unknown-linux-musl"));
    assert!(got.contains("x86_64-apple-darwin"));
    assert!(got.contains("aarch64-apple-darwin"));
}

#[test]
fn parser_handles_inline_array() {
    let toml = r#"
[workspace.metadata.dist]
cargo-dist-version = "0.25.1"
targets = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-apple-darwin", "aarch64-apple-darwin"]
pr-run-mode = "plan"
"#;
    let got = parse_dist_targets(toml);
    assert_eq!(got.len(), 4, "parsed {got:?}");
    assert!(got.contains("x86_64-unknown-linux-musl"));
    assert!(got.contains("aarch64-apple-darwin"));
}

#[test]
fn parser_anchored_to_dist_section_ignores_sibling_targets() {
    // The real Cargo.toml has a `targets = […]` inside the sub-table
    // `[workspace.metadata.dist.dependencies.apt]` (per-package cross-
    // compile sysroot list). Section-anchoring pins extraction to the
    // `[workspace.metadata.dist]` body proper.
    let toml = r#"
[workspace.metadata.dist]
targets = [
    "x86_64-unknown-linux-musl",
]

[workspace.metadata.dist.dependencies.apt]
gcc-aarch64-linux-gnu = { targets = ["aarch64-unknown-linux-gnu"] }
"#;
    let got = parse_dist_targets(toml);
    assert_eq!(got, BTreeSet::from(["x86_64-unknown-linux-musl".into()]));
    assert!(
        !got.contains("aarch64-unknown-linux-gnu"),
        "sibling sub-table's `targets = [`-shaped value leaked into the \
         cargo-dist asset list (#324 section-anchoring regression): {got:?}"
    );
}

#[test]
#[should_panic(expected = "no `[workspace.metadata.dist]` table in Cargo.toml")]
fn parser_panics_loudly_when_section_missing() {
    parse_dist_targets("[package]\nname = \"x\"\n");
}

#[test]
#[should_panic(expected = "no `targets = [`")]
fn parser_panics_loudly_when_array_missing() {
    parse_dist_targets("[workspace.metadata.dist]\ncargo-dist-version = \"0.25.1\"\n");
}
