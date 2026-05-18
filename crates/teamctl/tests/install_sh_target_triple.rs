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
    let cargo = read("Cargo.toml");
    let mut lines = cargo.lines();
    lines
        .by_ref()
        .find(|l| l.trim_start().starts_with("targets = ["))
        .unwrap_or_else(|| panic!("no `targets = [` array found in root Cargo.toml"));

    let mut out = BTreeSet::new();
    for line in lines {
        if line.contains(']') {
            break;
        }
        if let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) {
            if b > a {
                out.insert(line[a + 1..b].to_string());
            }
        }
    }
    out
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
