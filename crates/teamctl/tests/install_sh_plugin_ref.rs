//! Regression guard for #298.
//!
//! `tools/install.sh` is the single source of truth for the
//! `curl … | sh` installer (docs.yml regenerates `docs/public/install`
//! from it on every build). The Claude Code plugin step there must use
//! the marketplace-qualified id `teamctl@teamctl` for `plugin update` /
//! `plugin install` — the bare `teamctl` form fails unconditionally
//! with `Plugin "teamctl" not found`, which silently left every
//! installed plugin stale (#298).
//!
//! Exercising the real `claude plugin update` needs Claude Code + the
//! marketplace network round-trip, neither available in CI, so this is
//! a static-content guard instead: it pins the exact string the user
//! empirically confirmed works and the Rust CLI already canonicalizes
//! (`update.rs`'s `PLUGIN_ID = "teamctl@teamctl"`).

use std::fs;
use std::path::PathBuf;

fn install_sh() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tools/install.sh");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `claude plugin update` / `claude plugin install` in the
/// installer must name the plugin as `teamctl@teamctl`, never the
/// bare `teamctl`. `claude plugin marketplace update teamctl` is the
/// deliberate exception — that subcommand takes the bare *marketplace*
/// name, not a plugin id, so it is filtered out here on purpose.
#[test]
fn installer_uses_marketplace_qualified_plugin_id() {
    let src = install_sh();
    let mut checked = 0;

    for (lineno, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        // Only the plugin-id-taking subcommands. `marketplace update`
        // / `marketplace add` take the bare marketplace name / repo
        // spec respectively and are correctly bare.
        let is_plugin_id_cmd = (trimmed.contains("claude plugin update ")
            || trimmed.contains("claude plugin install "))
            && !trimmed.contains("plugin marketplace");
        if !is_plugin_id_cmd {
            continue;
        }
        checked += 1;
        assert!(
            line.contains("teamctl@teamctl"),
            "tools/install.sh:{} uses a non-marketplace-qualified plugin id \
             — `claude plugin update|install` requires `teamctl@teamctl`, \
             the bare `teamctl` form always fails (#298):\n  {line}",
            lineno + 1
        );
    }

    assert!(
        checked >= 2,
        "expected to find the plugin update + install invocations in \
         tools/install.sh; found {checked} — did the install flow move?"
    );
}
