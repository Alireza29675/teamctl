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
