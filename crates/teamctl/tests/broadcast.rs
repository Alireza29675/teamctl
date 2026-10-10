//! Exercise the real CLI adapter contract used by the TUI compose modal.

use std::{fs, path::Path, process::Command};

use rusqlite::Connection;
use tempfile::TempDir;

fn fixture(two_projects: bool) -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("projects")).unwrap();
    let mut projects = String::from("  - file: projects/one.yaml\n");
    if two_projects {
        projects.push_str("  - file: projects/two.yaml\n");
    }
    fs::write(
        tmp.path().join("team-compose.yaml"),
        format!(
            "version: 2.0.1\nbroker: {{type: sqlite, path: custom/mail.db}}\nprojects:\n{projects}"
        ),
    )
    .unwrap();
    for id in if two_projects {
        vec!["one", "two"]
    } else {
        vec!["one"]
    } {
        fs::write(
            tmp.path().join(format!("projects/{id}.yaml")),
            format!("version: 2\nproject: {{id: {id}, name: {id}, cwd: .}}\nchannels:\n  - name: all\n    members: '*'\nmanagers:\n  manager: {{runtime: codex}}\n"),
        )
        .unwrap();
    }
    tmp
}

fn broadcast(root: &Path, target: &str, body: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--root")
        .arg(root)
        .args(["broadcast", "--", target, body])
        .output()
        .unwrap()
}

#[test]
fn option_shaped_message_body_is_inserted_verbatim() {
    let tmp = fixture(false);
    for verb in ["broadcast", "send"] {
        let target = if verb == "send" {
            "one:manager"
        } else {
            "one:all"
        };
        let out = Command::new(env!("CARGO_BIN_EXE_teamctl"))
            .arg("--root")
            .arg(tmp.path())
            .args([verb, "--", target, "--help"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let conn = Connection::open(tmp.path().join("custom/mail.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE text = '--help'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn qualified_broadcast_preserves_project_and_literal_unicode_body() {
    let tmp = fixture(true);
    let body = "سلام 📎 'quoted' $(literal)\nsecond line";
    let out = broadcast(tmp.path(), "two:all", body);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let conn = Connection::open(tmp.path().join("custom/mail.db")).unwrap();
    let row: (String, String, String, String) = conn
        .query_row(
            "SELECT project_id, sender, recipient, text FROM messages",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "two".into(),
            "cli".into(),
            "channel:two:all".into(),
            body.into()
        )
    );
    assert!(!tmp.path().join("state/mailbox.db").exists());
}

#[test]
fn canonical_recipient_form_is_accepted() {
    let tmp = fixture(true);
    assert!(broadcast(tmp.path(), "channel:one:all", "hello")
        .status
        .success());
}

#[test]
fn bare_channel_is_allowed_when_unambiguous() {
    let tmp = fixture(false);
    assert!(broadcast(tmp.path(), "#all", "hello").status.success());
}

#[test]
fn ambiguous_broadcast_rejects_without_creating_mailbox() {
    let tmp = fixture(true);
    let out = broadcast(tmp.path(), "#all", "hello");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous channel"));
    assert!(!tmp.path().join("custom/mail.db").exists());
}

#[test]
fn unknown_channel_or_project_rejects_without_creating_mailbox() {
    let tmp = fixture(true);
    for target in ["one:missing", "missing:all", "", "one:"] {
        let out = broadcast(tmp.path(), target, "hello");
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("no such channel"));
    }
    assert!(!tmp.path().join("custom/mail.db").exists());
}
