//! `teamctl update` — re-run the installer that brought teamctl in.
//!
//! Detects the install method from `current_exe()`:
//!
//! - `…/Cellar/teamctl/…` → Homebrew (`brew upgrade teamctl`).
//! - `…/.cargo/bin/teamctl` → cargo (`cargo install teamctl team-mcp team-bot --force`).
//! - Anything else → shell installer (`curl -fsSL https://teamctl.run/install | sh`).
//!
//! The user can override autodetect with `--method <name>` and skip the
//! "Update? [Y/n]" prompt with `--yes`. `--check` prints the version
//! comparison and exits without updating.

use std::path::Path;
use std::process::{Command, ExitStatus};

use anyhow::{anyhow, bail, Context, Result};

const INSTALL_URL: &str = "https://teamctl.run/install";
const RELEASES_API: &str = "https://api.github.com/repos/Alireza29675/teamctl/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            return Ok(());
        }
        VersionOrder::Newer => {
            println!(
                "Local version {CURRENT_VERSION} is ahead of the latest published \
                 release ({latest}) — nothing to update."
            );
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
    Ok(())
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

fn fetch_latest_version() -> Result<String> {
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
enum VersionOrder {
    Older,
    Equal,
    Newer,
}

/// Lexicographic semver compare for `MAJOR.MINOR.PATCH`. Pre-release
/// suffixes (e.g. `-rc.1`) are stripped before comparison; we treat
/// them as equal to the base version for update-prompt purposes,
/// because anyone running a pre-release knowingly opted in.
fn compare_versions(local: &str, latest: &str) -> VersionOrder {
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
            InstallMethod::Cargo => "cargo install teamctl team-mcp team-bot --force".to_string(),
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
    let status = Command::new("brew")
        .args(["upgrade", "teamctl"])
        .status()
        .context("run `brew upgrade teamctl`")?;
    require_success(status, "brew upgrade teamctl")
}

fn exec_cargo_install() -> Result<()> {
    require_on_path("cargo")?;
    let status = Command::new("cargo")
        .args(["install", "teamctl", "team-mcp", "team-bot", "--force"])
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
    fn compare_versions_orders_correctly() {
        assert_eq!(compare_versions("0.5.1", "0.6.0"), VersionOrder::Older);
        assert_eq!(compare_versions("0.6.0", "0.6.0"), VersionOrder::Equal);
        assert_eq!(compare_versions("0.6.1", "0.6.0"), VersionOrder::Newer);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), VersionOrder::Newer);
        // Pre-release suffix is stripped → equal to its base version.
        assert_eq!(compare_versions("0.6.0-rc.1", "0.6.0"), VersionOrder::Equal);
    }
}
