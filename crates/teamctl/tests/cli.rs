//! End-to-end integration test for the `teamctl` binary.
//!
//! Intentionally avoids `tmux` + `claude` so it runs on CI without a TTY:
//! drives only `validate` and `send` (which talk to SQLite directly), then
//! walks the mailbox to confirm the message landed.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_teamctl").into()
}

fn seed_compose(root: &std::path::Path) {
    fs::write(
        root.join("team-compose.yaml"),
        r#"
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
supervisor:
  type: tmux
  tmux_prefix: a-
projects:
  - file: projects/hello.yaml
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(
        root.join("projects/hello.yaml"),
        r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
channels:
  - name: all
    members: "*"
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    telegram_inbox: true
    reports_to_user: true
    can_dm: [dev]
    can_broadcast: [all]
workers:
  dev:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager]
    can_broadcast: [all]
"#,
    )
    .unwrap();
}

#[test]
fn validate_passes_on_clean_compose() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());
    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("1 project"), "got: {stdout}");
    assert!(stdout.contains("2 agents"), "got: {stdout}");
}

#[test]
fn validate_fails_on_unknown_dm_target() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());
    let path = tmp.path().join("projects/hello.yaml");
    let contents = fs::read_to_string(&path)
        .unwrap()
        .replace("can_dm: [dev]", "can_dm: [ghost]");
    fs::write(&path, contents).unwrap();

    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("unknown agent `ghost`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn send_injects_into_mailbox() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());

    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "send",
            "hello:manager",
            "hi there",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let db = tmp.path().join("state/mailbox.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let (sender, recipient, text): (String, String, String) = conn
        .query_row(
            "SELECT sender, recipient, text FROM messages ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(sender, "cli");
    assert_eq!(recipient, "hello:manager");
    assert_eq!(text, "hi there");
}

// ── T-010: source-aware override warning ─────────────────────────────────

/// Run `teamctl validate` against `cwd` with a clean env, returning stderr.
/// `extra_env` lets each test inject the override under test (TEAMCTL_ROOT,
/// TEAMCTL_QUIET, ...). `home` isolates the registered-context store at
/// `$HOME/.config/teamctl/contexts.json`.
fn run_validate_with_env(
    cwd: &std::path::Path,
    home: &std::path::Path,
    extra_env: &[(&str, &str)],
    explicit_root: Option<&std::path::Path>,
) -> String {
    let mut cmd = Command::new(bin());
    cmd.env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(cwd);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    if let Some(r) = explicit_root {
        cmd.args(["--root", r.to_str().unwrap(), "validate"]);
    } else {
        cmd.arg("validate");
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "validate exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stderr).unwrap()
}

/// Lay out a `.team/`-style root at `<dir>/.team/` (so cwd walk-up will find it).
fn seed_dot_team(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join(".team");
    fs::create_dir_all(&root).unwrap();
    seed_compose(&root);
    root
}

/// Strip ANSI colour codes so assertions are stable regardless of TTY.
fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

#[test]
fn warn_a_walk_up_silent() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    let _ = root; // walk-up will find it from cwd
    let stderr = run_validate_with_env(tmp.path(), home.path(), &[], None);
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "walk-up must not warn; stderr was: {clean}"
    );
}

#[test]
fn warn_b_env_root_warns() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    // CWD is also a valid walk-up target — warning still fires because the
    // resolved root came from env, not walk-up.
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[("TEAMCTL_ROOT", root.to_str().unwrap())],
        None,
    );
    let clean = strip_ansi(&stderr);
    assert!(
        clean.contains("warning:") && clean.contains("TEAMCTL_ROOT"),
        "expected env warning; stderr was: {clean}"
    );
}

#[test]
fn warn_b_empty_env_root_treated_as_unset() {
    // `TEAMCTL_ROOT=""` (exported empty) should fall through to walk-up
    // rather than errorring on `canonicalize("")`.
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let _ = seed_dot_team(tmp.path());
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[("TEAMCTL_ROOT", "")],
        None,
    );
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "empty TEAMCTL_ROOT must fall through silently to walk-up; stderr was: {clean}"
    );
}

#[test]
fn warn_c_explicit_root_silent() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    // Even with TEAMCTL_ROOT in env, --root on the CLI is the deliberate intent.
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[("TEAMCTL_ROOT", "/definitely/not/this")],
        Some(&root),
    );
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "--root must not warn; stderr was: {clean}"
    );
}

#[test]
fn warn_d_registered_context_warns() {
    let tmp = tempdir().unwrap();
    let unrelated_cwd = tempdir().unwrap(); // no .team here, no walk-up hit
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());

    // Pre-populate the contexts store at $HOME/.config/teamctl/contexts.json.
    let cfg_dir = home.path().join(".config/teamctl");
    fs::create_dir_all(&cfg_dir).unwrap();
    let store = format!(
        r#"{{"current":"demo","contexts":{{"demo":"{}"}}}}"#,
        root.display()
    );
    fs::write(cfg_dir.join("contexts.json"), store).unwrap();

    let stderr = run_validate_with_env(unrelated_cwd.path(), home.path(), &[], None);
    let clean = strip_ansi(&stderr);
    assert!(
        clean.contains("warning:") && clean.contains("context 'demo'"),
        "expected context warning; stderr was: {clean}"
    );
}

#[test]
fn warn_e_quiet_silences_env() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[
            ("TEAMCTL_ROOT", root.to_str().unwrap()),
            ("TEAMCTL_QUIET", "1"),
        ],
        None,
    );
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "TEAMCTL_QUIET=1 must silence; stderr was: {clean}"
    );
}
