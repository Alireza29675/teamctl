//! `teamctl update` — re-run the installer that brought teamctl in.
//!
//! Detects the install method from `current_exe()`:
//!
//! - `…/Cellar/teamctl/…` → Homebrew (`brew upgrade teamctl`).
//! - `…/.cargo/bin/teamctl` → cargo (`cargo install teamctl teamctl-ui team-mcp team-bot --force`).
//! - Anything else → shell installer (`curl -fsSL https://teamctl.run/install | sh`).
//!
//! The user can override autodetect with `--method <name>` and skip the
//! "Update? [Y/n]" prompt with `--yes`. `--check` prints the version
//! comparison and exits without updating.

use std::path::Path;
use std::process::{Command, ExitStatus};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

const INSTALL_URL: &str = "https://teamctl.run/install";
const RELEASES_API: &str = "https://api.github.com/repos/Alireza29675/teamctl/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// T-146: Claude Code plugin id `teamctl@teamctl` (marketplace-qualified).
/// Hardcoded — the plugin lives at a fixed marketplace path the README
/// already documents; v1 doesn't grow a config knob for this.
const PLUGIN_ID: &str = "teamctl@teamctl";

/// T-188: single source of truth for the workspace binaries `teamctl
/// update` reinstalls on the cargo path. Used by both the displayed
/// "About to run:" preview and the actual `cargo install` exec call,
/// so the two can't drift the way they did when `teamctl-ui` was
/// silently missing from the exec list. Order matches the shape
/// `tools/install.sh:91` already ships with so user-visible commands
/// look identical across install paths.
const CARGO_INSTALL_CRATES: &[&str] = &["teamctl", "teamctl-ui", "team-mcp", "team-bot"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Shell,
    Homebrew,
    Cargo,
}

impl InstallMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallMethod::Shell => "shell",
            InstallMethod::Homebrew => "brew",
            InstallMethod::Cargo => "cargo",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "shell" | "installer" | "curl" => Ok(InstallMethod::Shell),
            "brew" | "homebrew" => Ok(InstallMethod::Homebrew),
            "cargo" => Ok(InstallMethod::Cargo),
            other => bail!("unknown method `{other}` (expected: shell, brew, cargo)"),
        }
    }
}

pub fn run(method_override: Option<String>, check_only: bool, yes: bool) -> Result<()> {
    let exe = std::env::current_exe().context("locate current teamctl exe")?;
    let detected = detect_install_method(&exe);
    let method = match method_override.as_deref() {
        Some(s) => InstallMethod::parse(s)?,
        None => detected,
    };

    println!(
        "teamctl {CURRENT_VERSION} ({} install, exe: {})",
        method.as_str(),
        exe.display()
    );

    let latest = fetch_latest_version()
        .with_context(|| format!("fetch latest version from {RELEASES_API}"))?;
    let cmp = compare_versions(CURRENT_VERSION, &latest);

    match cmp {
        VersionOrder::Equal => {
            println!("✓ already on the latest version ({latest}).");
            // T-146: even a no-op binary update keeps the plugin paired
            // with the binary — flip a marketplace-side bump in the same
            // command. `--check` skips because it is the version-probe
            // mode, not an update path.
            if !check_only {
                try_update_claude_plugin(&RealClaudeRunner);
            }
            return Ok(());
        }
        VersionOrder::Newer => {
            println!(
                "Local version {CURRENT_VERSION} is ahead of the latest published \
                 release ({latest}) — nothing to update."
            );
            // Local-ahead is a dev-machine state, not a successful
            // update; skip the plugin step to avoid surprising a
            // contributor running an unreleased build.
            return Ok(());
        }
        VersionOrder::Older => {
            println!("→ update available: {CURRENT_VERSION} → {latest}");
        }
    }

    if check_only {
        return Ok(());
    }

    let plan = plan_for(method);
    println!("Plan: {}", plan.describe());

    if !yes && !confirm("Proceed? [Y/n] ", true)? {
        println!("  cancelled");
        return Ok(());
    }

    plan.execute()?;
    println!("✓ update complete. Run `teamctl --version` to confirm.");
    try_update_claude_plugin(&RealClaudeRunner);
    // Don't dump the whole changelog inline on update — point the
    // operator at `teamctl whatsnew --since <old>`, which renders the
    // release notes since <old> on demand. <old> is the version we just
    // updated from, pre-filled so the command is copy-paste ready.
    // Leading blank line separates the nudge from the installer output. (#302)
    println!();
    println!("{}", whatsnew_nudge(CURRENT_VERSION));
    Ok(())
}

/// The post-update nudge (#302): instead of dumping the whole changelog
/// inline, point the operator at `teamctl whatsnew --since <from>`, which
/// renders the release notes since `from` on demand. `from` is the version
/// we just updated away from, pre-filled so the command is copy-paste
/// ready. Kept as a pure fn so the wiring is unit-testable without a live
/// update.
fn whatsnew_nudge(from: &str) -> String {
    format!("To see what's new, run: `teamctl whatsnew --since {from}`")
}

// ── Detection ───────────────────────────────────────────────────────

/// Pick an install method from the path of the running `teamctl` exe.
/// Robust to both macOS (`/opt/homebrew/Cellar/...`) and Linux brew
/// (`/home/linuxbrew/...`) layouts. Returns Shell for anything we
/// don't recognise — that's also what the install.sh-managed
/// `~/.local/bin` path looks like.
pub fn detect_install_method(exe: &Path) -> InstallMethod {
    let p = exe.to_string_lossy();
    if p.contains("/Cellar/teamctl/") || p.contains("/linuxbrew/") || p.contains("/homebrew/") {
        return InstallMethod::Homebrew;
    }
    if p.contains("/.cargo/bin/") || p.contains("/cargo/bin/") {
        return InstallMethod::Cargo;
    }
    InstallMethod::Shell
}

// ── Latest-version probe ────────────────────────────────────────────

pub(crate) fn fetch_latest_version() -> Result<String> {
    // GitHub's releases API returns JSON. We only need `tag_name`, and
    // the response can be either pretty-printed (line per field) or a
    // single 80kB blob, so we scan the body as a whole string rather
    // than line-by-line.
    let body = curl_get(RELEASES_API)?;
    extract_tag_name(&body)
        .ok_or_else(|| anyhow!("no `tag_name` field in GitHub releases response"))
}

/// Pull the `tag_name` value out of a GitHub releases-API JSON blob.
/// Returns `None` when the field isn't present or is empty. Strips a
/// leading `v` so callers can compare directly against `Cargo.toml`'s
/// version.
fn extract_tag_name(body: &str) -> Option<String> {
    let needle = "\"tag_name\":";
    let idx = body.find(needle)?;
    let after = &body[idx + needle.len()..];
    let after = after.trim_start();
    let value = after.strip_prefix('"')?;
    let end = value.find('"')?;
    let tag = value[..end].trim().trim_start_matches('v').to_string();
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VersionOrder {
    Older,
    Equal,
    Newer,
}

/// Lexicographic semver compare for `MAJOR.MINOR.PATCH`. Pre-release
/// suffixes (e.g. `-rc.1`) are stripped before comparison; we treat
/// them as equal to the base version for update-prompt purposes,
/// because anyone running a pre-release knowingly opted in.
pub(crate) fn compare_versions(local: &str, latest: &str) -> VersionOrder {
    let l = parse_triplet(local);
    let r = parse_triplet(latest);
    match l.cmp(&r) {
        std::cmp::Ordering::Less => VersionOrder::Older,
        std::cmp::Ordering::Equal => VersionOrder::Equal,
        std::cmp::Ordering::Greater => VersionOrder::Newer,
    }
}

fn parse_triplet(v: &str) -> (u32, u32, u32) {
    let core = v.split('-').next().unwrap_or(v).trim_start_matches('v');
    let mut it = core.split('.');
    let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

// ── Install plan ────────────────────────────────────────────────────

struct Plan {
    method: InstallMethod,
}

impl Plan {
    fn describe(&self) -> String {
        match self.method {
            InstallMethod::Shell => format!("re-run the shell installer (curl {INSTALL_URL} | sh)"),
            InstallMethod::Homebrew => "brew upgrade teamctl".to_string(),
            InstallMethod::Cargo => {
                format!("cargo install {} --force", CARGO_INSTALL_CRATES.join(" "))
            }
        }
    }

    fn execute(&self) -> Result<()> {
        match self.method {
            InstallMethod::Shell => exec_shell_installer(),
            InstallMethod::Homebrew => exec_brew_upgrade(),
            InstallMethod::Cargo => exec_cargo_install(),
        }
    }
}

fn plan_for(method: InstallMethod) -> Plan {
    Plan { method }
}

fn exec_shell_installer() -> Result<()> {
    require_on_path("curl")?;
    require_on_path("sh")?;
    // We pipe curl into sh via a single shell invocation so the
    // installer's progress output streams to the user in real time.
    let cmd = format!("curl -fsSL {INSTALL_URL} | sh");
    let status = Command::new("sh")
        .args(["-c", &cmd])
        .status()
        .context("run shell installer")?;
    require_success(status, "shell installer")
}

fn exec_brew_upgrade() -> Result<()> {
    require_on_path("brew")?;
    // `brew update` first so the formula bump from cargo-dist's
    // homebrew tap is picked up; otherwise `brew upgrade` may report
    // "already up-to-date" against a stale tap.
    let status = Command::new("brew")
        .args(["update"])
        .status()
        .context("run `brew update`")?;
    require_success(status, "brew update")?;
    // T-188: the homebrew tap is currently disabled (#3); if/when it
    // re-enables with separate `teamctl-ui` / `team-mcp` / `team-bot`
    // formulae, this needs to upgrade all of them — mirror the
    // `CARGO_INSTALL_CRATES` list above — or the brew path will leak
    // the same staleness bug the cargo path just shed.
    let status = Command::new("brew")
        .args(["upgrade", "teamctl"])
        .status()
        .context("run `brew upgrade teamctl`")?;
    require_success(status, "brew upgrade teamctl")
}

fn exec_cargo_install() -> Result<()> {
    require_on_path("cargo")?;
    let status = Command::new("cargo")
        .arg("install")
        .args(CARGO_INSTALL_CRATES)
        .arg("--force")
        .status()
        .context("run cargo install")?;
    require_success(status, "cargo install")
}

fn require_on_path(bin: &str) -> Result<()> {
    if which(bin).is_some() {
        return Ok(());
    }
    bail!("`{bin}` not found on PATH — install it and re-run, or pick another method with --method")
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe_candidate = candidate.with_extension("exe");
            if exe_candidate.is_file() {
                return Some(exe_candidate);
            }
        }
    }
    None
}

fn require_success(status: ExitStatus, label: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    bail!(
        "{label} exited with status {} — see output above",
        status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "(killed by signal)".into())
    )
}

fn curl_get(url: &str) -> Result<String> {
    // Mirror the helper in cmd::bot — keeps deps minimal. GitHub's API
    // requires a User-Agent header, so we set one explicitly; without
    // it we get a 403 even for unauthenticated read endpoints.
    let out = Command::new("curl")
        .args([
            "-sS",
            "-H",
            &format!("User-Agent: teamctl-cli/{CURRENT_VERSION}"),
            "--max-time",
            "15",
            url,
        ])
        .output()
        .context("run curl (is curl installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("curl failed: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn confirm(msg: &str, default_yes: bool) -> Result<bool> {
    use std::io::{self, BufRead, Write};
    print!("{msg}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read stdin")?;
    let raw = line.trim().to_lowercase();
    if raw.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(raw.as_str(), "y" | "yes"))
}

// ── Claude Code plugin sync (T-146) ────────────────────────────────

/// External-`claude` abstraction. Production impl shells out via
/// `Command::new("claude")`; tests inject a mock that returns canned
/// stdout without spawning a subprocess. Two methods so the
/// PATH-probe stays cheap (no spawn) and the JSON-list/update calls
/// share a single shape.
trait ClaudeRunner {
    /// `true` when `claude` is on PATH. Lets the post-update hook
    /// short-circuit silently for users who haven't installed Claude
    /// Code — they're not on the recommended path; no warning needed.
    fn is_claude_on_path(&self) -> bool;

    /// Run `claude <args...>`. Returns `Ok(stdout)` on exit 0, `Err`
    /// on spawn-failure or non-zero exit. Note: `claude plugin update`
    /// can print failures to stdout while still exiting 0 — callers
    /// must inspect stdout for the `✘` glyph or "Failed to update"
    /// substring.
    fn run(&self, args: &[&str]) -> Result<String>;
}

struct RealClaudeRunner;

impl ClaudeRunner for RealClaudeRunner {
    fn is_claude_on_path(&self) -> bool {
        which("claude").is_some()
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("claude")
            .args(args)
            .output()
            .with_context(|| format!("run `claude {}`", args.join(" ")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!(
                "claude {} exited with status {} ({})",
                args.join(" "),
                out.status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "(killed by signal)".into()),
                stderr.trim(),
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Try to update the teamctl Claude Code plugin after a successful
/// binary update. Skip-silent when `claude` is not on PATH or the
/// plugin isn't installed (recommended-path opt-out shapes — no
/// warning). On a real plugin-update failure, print a quiet one-liner
/// with the manual-fix hint and return — the binary update is never
/// rolled back, per the ticket's explicit contract.
fn try_update_claude_plugin(runner: &dyn ClaudeRunner) {
    if !runner.is_claude_on_path() {
        return;
    }
    let listed = match runner.run(&["plugin", "list", "--json"]) {
        Ok(s) => s,
        Err(_) => return,
    };
    if !plugin_installed(&listed, PLUGIN_ID) {
        return;
    }
    match runner.run(&["plugin", "update", PLUGIN_ID]) {
        Ok(stdout) => println!("{}", summarize_plugin_update(&stdout, PLUGIN_ID)),
        Err(e) => println!(
            "! claude plugin update failed: {e} — run `claude plugin update {PLUGIN_ID}` manually."
        ),
    }
}

/// Returns `true` when `id` appears as the `id` field of any object in
/// the JSON array `claude plugin list --json` produces. Tolerant of
/// unparseable JSON — returns `false` rather than erroring so the
/// post-update hook stays silent on malformed output (operator's
/// claude version may have a different output shape we don't yet
/// know about).
fn plugin_installed(list_json: &str, id: &str) -> bool {
    let v: Value = match serde_json::from_str(list_json) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.as_array()
        .map(|arr| {
            arr.iter()
                .any(|p| p.get("id").and_then(|s| s.as_str()) == Some(id))
        })
        .unwrap_or(false)
}

/// Convert the multi-line stdout of `claude plugin update` into a
/// single line in teamctl-style. Three shapes claude can print:
/// - `✔ teamctl is already at the latest version (0.1.0).` — no-op.
/// - `✔ Updated teamctl to <ver>.` (or similar) — real update.
/// - `✘ Failed to update plugin "<id>": <reason>` — failure (claude
///   exits 0 in this case, so the glyph is the only signal).
fn summarize_plugin_update(stdout: &str, id: &str) -> String {
    let body = stdout.trim();
    if body.contains('✘') || body.contains("Failed to update") {
        let reason = body
            .lines()
            .find(|l| l.contains('✘') || l.contains("Failed to update"))
            .unwrap_or(body)
            .trim();
        return format!("! {reason} — run `claude plugin update {id}` manually.");
    }
    if body.contains("already at the latest version") {
        return format!("✓ claude plugin {id} already current.");
    }
    format!("✓ claude plugin {id} updated.")
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_homebrew_macos_layout() {
        let exe = PathBuf::from("/opt/homebrew/Cellar/teamctl/0.6.0/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Homebrew);
    }

    #[test]
    fn detect_homebrew_intel_layout() {
        let exe = PathBuf::from("/usr/local/Cellar/teamctl/0.6.0/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Homebrew);
    }

    #[test]
    fn detect_homebrew_linux_layout() {
        let exe = PathBuf::from("/home/linuxbrew/.linuxbrew/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Homebrew);
    }

    #[test]
    fn detect_cargo_layout() {
        let exe = PathBuf::from("/Users/alireza/.cargo/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Cargo);
    }

    #[test]
    fn detect_shell_default_for_local_bin() {
        let exe = PathBuf::from("/Users/alireza/.local/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Shell);
    }

    #[test]
    fn detect_shell_default_for_unknown_path() {
        let exe = PathBuf::from("/opt/teamctl/bin/teamctl");
        assert_eq!(detect_install_method(&exe), InstallMethod::Shell);
    }

    #[test]
    fn parse_method_accepts_synonyms() {
        assert_eq!(InstallMethod::parse("shell").unwrap(), InstallMethod::Shell);
        assert_eq!(InstallMethod::parse("curl").unwrap(), InstallMethod::Shell);
        assert_eq!(
            InstallMethod::parse("installer").unwrap(),
            InstallMethod::Shell
        );
        assert_eq!(
            InstallMethod::parse("brew").unwrap(),
            InstallMethod::Homebrew
        );
        assert_eq!(
            InstallMethod::parse("Homebrew").unwrap(),
            InstallMethod::Homebrew
        );
        assert_eq!(InstallMethod::parse("cargo").unwrap(), InstallMethod::Cargo);
    }

    #[test]
    fn parse_method_rejects_garbage() {
        assert!(InstallMethod::parse("snap").is_err());
        assert!(InstallMethod::parse("").is_err());
    }

    #[test]
    fn parse_triplet_handles_v_prefix_and_pre() {
        assert_eq!(parse_triplet("0.6.0"), (0, 6, 0));
        assert_eq!(parse_triplet("v0.6.0"), (0, 6, 0));
        assert_eq!(parse_triplet("1.2.3-rc.1"), (1, 2, 3));
        assert_eq!(parse_triplet("v10.20.30"), (10, 20, 30));
    }

    #[test]
    fn extract_tag_name_handles_single_line_json() {
        let blob = r#"{"id":1,"tag_name":"v0.6.0","name":"0.6.0"}"#;
        assert_eq!(extract_tag_name(blob).as_deref(), Some("0.6.0"));
    }

    #[test]
    fn extract_tag_name_handles_pretty_printed_json() {
        let blob = "{\n  \"id\": 1,\n  \"tag_name\": \"v0.5.1\",\n  \"name\": \"0.5.1\"\n}";
        assert_eq!(extract_tag_name(blob).as_deref(), Some("0.5.1"));
    }

    #[test]
    fn extract_tag_name_strips_v_prefix() {
        let blob = r#"{"tag_name":"v10.20.30"}"#;
        assert_eq!(extract_tag_name(blob).as_deref(), Some("10.20.30"));
    }

    #[test]
    fn extract_tag_name_returns_none_for_missing_field() {
        let blob = r#"{"message":"Not Found","status":"404"}"#;
        assert!(extract_tag_name(blob).is_none());
    }

    #[test]
    fn extract_tag_name_returns_none_for_empty_value() {
        let blob = r#"{"tag_name":""}"#;
        assert!(extract_tag_name(blob).is_none());
    }

    #[test]
    fn cargo_install_crates_includes_all_four_binaries() {
        // T-188: regression pin. The bug was that `teamctl-ui` silently
        // dropped out of the cargo install list, leaving operators with
        // a stale TUI binary across upgrades. The fix centralizes the
        // crate list as a const used by both `describe()` and
        // `exec_cargo_install()`, so they can't drift apart again.
        // Anchoring the const directly here catches a re-introduction.
        assert_eq!(
            CARGO_INSTALL_CRATES,
            &["teamctl", "teamctl-ui", "team-mcp", "team-bot"]
        );
    }

    #[test]
    fn cargo_plan_describe_lists_teamctl_ui() {
        // Belt-and-suspenders pin: the user-facing "About to run:"
        // string MUST surface `teamctl-ui`. If a future refactor
        // disconnects `describe()` from `CARGO_INSTALL_CRATES`, this
        // fails before the docstring/display drift would surface to
        // operators.
        let plan = Plan {
            method: InstallMethod::Cargo,
        };
        let line = plan.describe();
        assert!(
            line.contains("teamctl-ui"),
            "describe() must surface teamctl-ui, got: {line}"
        );
        assert!(line.contains("--force"));
    }

    #[test]
    fn compare_versions_orders_correctly() {
        assert_eq!(compare_versions("0.5.1", "0.6.0"), VersionOrder::Older);
        assert_eq!(compare_versions("0.6.0", "0.6.0"), VersionOrder::Equal);
        assert_eq!(compare_versions("0.6.1", "0.6.0"), VersionOrder::Newer);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), VersionOrder::Newer);
        // Pre-release suffix is stripped → equal to its base version.
        assert_eq!(compare_versions("0.6.0-rc.1", "0.6.0"), VersionOrder::Equal);
    }

    #[test]
    fn post_update_nudge_points_at_whatsnew_since_not_a_dump() {
        // #302: after `teamctl update` we no longer dump the full
        // changelog inline — we print a single nudge line pointing at
        // `teamctl whatsnew --since <from>`, with the from-version
        // pre-filled so it's copy-paste ready.
        let line = whatsnew_nudge("0.9.0");
        assert!(
            line.contains("teamctl whatsnew --since 0.9.0"),
            "nudge must surface the pre-filled whatsnew command, got: {line}"
        );
        assert!(
            !line.contains('\n'),
            "nudge must be a single line, not a changelog dump, got: {line}"
        );
    }

    // ── T-146 plugin sync ─────────────────────────────────────────

    use std::cell::RefCell;

    /// Test mock — records every `run()` arg vector and returns canned
    /// outputs in order. Lets each test pin both the call sequence and
    /// what was returned for it without spawning a real `claude`.
    struct MockRunner {
        on_path: bool,
        calls: RefCell<Vec<Vec<String>>>,
        responses: RefCell<Vec<Result<String>>>,
    }

    impl MockRunner {
        fn new(on_path: bool, responses: Vec<Result<String>>) -> Self {
            Self {
                on_path,
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses),
            }
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl ClaudeRunner for MockRunner {
        fn is_claude_on_path(&self) -> bool {
            self.on_path
        }
        fn run(&self, args: &[&str]) -> Result<String> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            self.responses
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| Err(anyhow!("mock exhausted")))
        }
    }

    #[test]
    fn plugin_step_skips_silent_when_claude_not_on_path() {
        // Recommended-path opt-out: user hasn't installed Claude Code.
        // The hook must not call `run()` at all — no spawn, no log.
        let runner = MockRunner::new(false, vec![]);
        try_update_claude_plugin(&runner);
        assert!(runner.calls().is_empty(), "expected zero claude calls");
    }

    #[test]
    fn plugin_step_skips_silent_when_plugin_not_installed() {
        // Claude is on PATH but the user runs the rust-analyzer plugin
        // only — the teamctl plugin id must not appear in the JSON
        // array. After the list call, no update call should fire.
        let other_plugin_only = r#"[
            {"id":"rust-analyzer-lsp@claude-plugins-official","version":"1.0.0"}
        ]"#;
        let runner = MockRunner::new(true, vec![Ok(other_plugin_only.to_string())]);
        try_update_claude_plugin(&runner);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "only the list call should fire");
        assert_eq!(calls[0], vec!["plugin", "list", "--json"]);
    }

    #[test]
    fn plugin_step_runs_update_when_plugin_installed() {
        // Happy path: detection finds teamctl@teamctl, update fires.
        // Stack the responses LIFO since MockRunner pops; first
        // element popped is the LAST in the vec.
        let installed_json = r#"[
            {"id":"teamctl@teamctl","version":"0.1.0"}
        ]"#;
        let update_stdout = "Checking for updates for plugin \"teamctl@teamctl\" at user scope…\n\
             ✔ teamctl is already at the latest version (0.1.0).\n";
        let runner = MockRunner::new(
            true,
            vec![
                Ok(update_stdout.to_string()),
                Ok(installed_json.to_string()),
            ],
        );
        try_update_claude_plugin(&runner);
        let calls = runner.calls();
        assert_eq!(calls.len(), 2, "list + update fired");
        assert_eq!(calls[1], vec!["plugin", "update", "teamctl@teamctl"]);
    }

    #[test]
    fn plugin_installed_finds_id_in_list() {
        let json = r#"[
            {"id":"rust-analyzer-lsp@claude-plugins-official","version":"1.0.0"},
            {"id":"teamctl@teamctl","version":"0.1.0"}
        ]"#;
        assert!(plugin_installed(json, "teamctl@teamctl"));
        assert!(!plugin_installed(json, "absent@somewhere"));
    }

    #[test]
    fn plugin_installed_handles_empty_list() {
        assert!(!plugin_installed("[]", "teamctl@teamctl"));
    }

    #[test]
    fn plugin_installed_returns_false_on_malformed_json() {
        // Tolerant: a future claude version that changes output shape
        // (top-level object, paged response) must not crash teamctl —
        // the post-update hook stays silent and the binary update
        // is unaffected.
        assert!(!plugin_installed("{not json", "teamctl@teamctl"));
        assert!(!plugin_installed(r#"{"plugins":[]}"#, "teamctl@teamctl"));
    }

    #[test]
    fn summarize_plugin_update_already_current() {
        let stdout = "Checking for updates…\n✔ teamctl is already at the latest version (0.1.0).\n";
        let line = summarize_plugin_update(stdout, "teamctl@teamctl");
        assert!(line.starts_with("✓"), "success glyph: {line}");
        assert!(line.contains("already current"), "{line}");
    }

    #[test]
    fn summarize_plugin_update_real_update() {
        // Simulate a hypothetical "Updated to" success line — claude's
        // exact wording isn't guaranteed across versions, so the
        // detector is "doesn't contain failure markers AND doesn't say
        // already-current → updated".
        let stdout = "✔ Updated teamctl to 0.1.1.";
        let line = summarize_plugin_update(stdout, "teamctl@teamctl");
        assert!(line.starts_with("✓"));
        assert!(line.contains("updated"), "{line}");
        assert!(!line.contains("already"), "{line}");
    }

    #[test]
    fn summarize_plugin_update_failure_glyph() {
        // claude exits 0 even on "Failed to update" — the glyph is
        // the only reliable signal. Fallback hint must include the
        // manual command so the user has a recovery path without
        // running `claude plugin --help`.
        let stdout =
            "Checking for updates…\n✘ Failed to update plugin \"teamctl@teamctl\": Plugin offline";
        let line = summarize_plugin_update(stdout, "teamctl@teamctl");
        assert!(line.starts_with("!"), "warn glyph: {line}");
        assert!(line.contains("offline") || line.contains("Failed"));
        assert!(
            line.contains("claude plugin update teamctl@teamctl"),
            "manual-fix hint missing: {line}"
        );
    }

    #[test]
    fn summarize_plugin_update_failure_text_without_glyph() {
        // Defensive: a future claude version may stop using the glyph
        // and just print the "Failed to update" text. Still detect.
        let stdout = "Failed to update plugin \"teamctl@teamctl\": network error";
        let line = summarize_plugin_update(stdout, "teamctl@teamctl");
        assert!(line.starts_with("!"));
        assert!(line.contains("network error"));
    }
}
