//! Regression guard + behavioral check for #262.
//!
//! `tools/install.sh` gates the Claude Code plugin install/update step
//! on a minimum Claude Code version. Pre-#262 the installer offered the
//! plugin regardless of version; older Claude Code releases have
//! substrate bugs (T-118 / T-174 lineage on `--session-id` resume) that
//! turn the first `teamctl up` into a confusing failure when the plugin
//! is installed against them. The owner-ratified floor is 2.1.141.
//!
//! This test covers two layers:
//!
//! 1. **Static-content guards** — the floor constant exists, the helper
//!    functions exist, the remediation message names the floor, and
//!    the plugin install/prompt arms are reached only via the
//!    floor-gate cascade (no orphan invocation that bypasses it).
//! 2. **Behavioral guard on `version_ge`** — extract the helper
//!    function definitions from `tools/install.sh` and exercise the
//!    three documented cases (`2.1.139` skip, `2.1.141` pass, `2.2.0`
//!    pass) in an isolated `sh -c`. Pins the issue's "Tests / smoke
//!    check" acceptance criterion at the function-correctness level
//!    without needing a real `claude` binary or a downloaded release.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn install_sh_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tools/install.sh")
}

fn install_sh() -> String {
    let path = install_sh_path();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Pull out a shell function body by name. The script defines functions
/// in the `name() { … }` form with the closing brace on its own line —
/// we match the contract literally to keep the parser shallow.
fn extract_fn(src: &str, name: &str) -> String {
    let header = format!("{name}() {{");
    let start = src.find(&header).unwrap_or_else(|| {
        panic!("expected function `{name}()` in tools/install.sh — did the helper move or rename?")
    });
    // Function ends at the first line that is exactly `}`.
    let after_open = &src[start..];
    let end_rel = after_open
        .lines()
        .scan(0usize, |acc, line| {
            let here = *acc;
            *acc += line.len() + 1; // +1 for the `\n`
            Some((here, line))
        })
        .find_map(|(off, line)| (line == "}").then_some(off + 1))
        .unwrap_or_else(|| panic!("no closing `}}` for `{name}()` in tools/install.sh"));
    src[start..start + end_rel].to_string()
}

#[test]
fn floor_constant_pins_2_1_141() {
    let src = install_sh();
    assert!(
        src.contains("CLAUDE_CODE_PLUGIN_FLOOR=\"2.1.141\""),
        "tools/install.sh must pin CLAUDE_CODE_PLUGIN_FLOOR=\"2.1.141\" \
         (#262 owner-ratified floor). Bumping requires updating the \
         release notes and the issue."
    );
}

#[test]
fn floor_skip_message_names_floor_and_remediation() {
    // The skip-path message must (a) name the floor + installed version
    // so the operator can act on it, (b) point at the canonical update
    // path, and (c) point at the re-run command. Verbatim per the issue
    // body — these strings are the public-facing contract.
    let src = install_sh();
    let must_contain = [
        // Names the floor literal AND interpolates the installed version.
        "teamctl Claude Code plugin requires Claude Code >= $CLAUDE_CODE_PLUGIN_FLOOR (installed: $installed_claude_version).",
        "Please update Claude Code first:",
        "claude --upgrade",
        "Then re-run the teamctl installer:",
        "curl -fsSL https://teamctl.run/install | sh",
    ];
    for needle in must_contain {
        assert!(
            src.contains(needle),
            "tools/install.sh missing required remediation-message fragment for \
             the #262 floor skip path:\n  expected substring: {needle}"
        );
    }
}

#[test]
fn version_check_helpers_exist() {
    // The two helpers form the contract surface tested below.
    // `claude_installed_version` parses `claude --version` output;
    // `version_ge` compares semvers. Both are pinned by name here so a
    // rename of either would require updating this test alongside,
    // which is the right tradeoff: rename forces re-verification of
    // the call-site cascade.
    let src = install_sh();
    assert!(
        src.contains("claude_installed_version() {"),
        "tools/install.sh missing `claude_installed_version()` helper (#262)"
    );
    assert!(
        src.contains("version_ge() {"),
        "tools/install.sh missing `version_ge()` helper (#262)"
    );
}

#[test]
fn plugin_install_arms_are_floor_gated() {
    // The two plugin-touching invocations (`claude plugin install
    // teamctl@teamctl` and `claude plugin update teamctl@teamctl`) must
    // ONLY be reachable through the floor-gate cascade. They live
    // inside the `elif plugin_installed` / `case y|Y` arms of the
    // cascade that begins with the `version_ge` check — so the
    // structural invariant is: every `claude plugin install|update
    // teamctl@teamctl` line in install.sh sits inside the
    // `if command -v claude` block AFTER a `version_ge` reference.
    //
    // We pin this by anchor: the `version_ge` invocation in the cascade
    // appears exactly once and BEFORE every `claude plugin install` /
    // `claude plugin update teamctl@teamctl` line.
    let src = install_sh();
    let version_ge_invocation = src.find("version_ge \"$installed_claude_version\"").expect(
        "tools/install.sh must invoke `version_ge \"$installed_claude_version\"` in the \
             Claude Code plugin gate cascade (#262); no invocation found",
    );
    for needle in [
        "claude plugin install teamctl@teamctl",
        "claude plugin update teamctl@teamctl",
    ] {
        let at = src.find(needle).unwrap_or_else(|| {
            panic!("expected `{needle}` in tools/install.sh — plugin flow moved?")
        });
        assert!(
            at > version_ge_invocation,
            "`{needle}` in tools/install.sh appears BEFORE the floor-gate \
             `version_ge` check — orphan invocation would bypass the #262 \
             gate. Move it inside the elif cascade or remove."
        );
    }
}

/// Run `version_ge` in an isolated shell with the function body
/// extracted from `tools/install.sh`. Returns the function's exit
/// status (true = installed >= floor).
fn version_ge_in_shell(installed: &str, floor: &str) -> bool {
    let src = install_sh();
    let body = extract_fn(&src, "version_ge");
    // Quote the args via single-quote escaping so a future input that
    // happens to contain a quote can't escape the test invocation.
    // (Semvers don't contain quotes today; defensive anyway.)
    let q_installed = installed.replace('\'', r#"'\''"#);
    let q_floor = floor.replace('\'', r#"'\''"#);
    let script = format!("{body}\nversion_ge '{q_installed}' '{q_floor}'\n");
    let status = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .status()
        .expect("spawn sh");
    status.success()
}

#[test]
fn version_ge_documented_cases() {
    // Issue acceptance criterion verbatim:
    //   "stubbed `claude --version` returning `2.1.139` triggers the
    //    skip path; `2.1.141` and `2.2.0` both pass."
    // We exercise `version_ge` directly with the documented inputs —
    // the function body is the only thing that decides skip vs pass.
    assert!(
        !version_ge_in_shell("2.1.139", "2.1.141"),
        "2.1.139 must be < 2.1.141 (skip path triggers)"
    );
    assert!(
        version_ge_in_shell("2.1.141", "2.1.141"),
        "2.1.141 must be >= 2.1.141 (equal passes)"
    );
    assert!(
        version_ge_in_shell("2.2.0", "2.1.141"),
        "2.2.0 must be >= 2.1.141 (clear major-floor pass)"
    );
}

#[test]
fn version_ge_lexical_pitfall_is_not_lexical() {
    // `sort -V` is the version-aware sort; plain string compare would
    // claim "2.1.9" > "2.1.10" because '9' > '1'. This pins that the
    // helper does the version-aware compare, not lexical — so a future
    // accidental swap to `sort` (no -V) would surface immediately.
    assert!(
        version_ge_in_shell("2.1.10", "2.1.9"),
        "2.1.10 must be >= 2.1.9 (version-aware compare, not lexical — \
         lexical would mis-order with '9' > '1')"
    );
    assert!(
        !version_ge_in_shell("2.1.9", "2.1.10"),
        "2.1.9 must be < 2.1.10 (symmetric of the previous case)"
    );
}
