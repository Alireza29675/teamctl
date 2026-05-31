//! `teamctl --version` baked-version format (#359).
//!
//! The exact string is environment-dependent — a dev build off a checkout
//! with tags shows the `git describe` form (`0.8.6-55-ge4adf50[-dirty]`); a
//! shallow/tag-less CI checkout or a no-git build falls back to the plain
//! `CARGO_PKG_VERSION` (`0.8.6`); a clean tagged release shows the bare tag
//! (`0.8.7`). What holds across *all* of those — and what this pins — is the
//! normalized shape: `teamctl <MAJOR.MINOR.PATCH…>` with no leading `v` and a
//! dotted numeric base (never a bare commit hash).

use std::process::Command;

fn version_token() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--version")
        .output()
        .expect("run `teamctl --version`");
    assert!(out.status.success(), "`--version` exited non-zero");
    let stdout = String::from_utf8(out.stdout).expect("utf8 --version output");
    stdout
        .trim()
        .strip_prefix("teamctl ")
        .expect("`teamctl <version>` shape")
        .to_string()
}

#[test]
fn version_has_no_v_prefix() {
    let v = version_token();
    assert!(
        !v.starts_with('v'),
        "the leading `v` must be stripped so dev/release formats match: {v}"
    );
}

#[test]
fn version_base_is_dotted_numeric_semver() {
    let v = version_token();
    // The base (before any `-NN-g…`/`-dirty` describe suffix) is always a
    // dotted MAJOR.MINOR.PATCH — true for the describe form and the
    // CARGO_PKG_VERSION fallback alike, never a bare `--always` commit hash.
    let base = v.split('-').next().expect("non-empty version");
    let parts: Vec<&str> = base.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "version base must be MAJOR.MINOR.PATCH: {v}"
    );
    assert!(
        parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "version base components must be numeric: {v}"
    );
}
