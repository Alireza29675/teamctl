use rusqlite::{params, Connection};
use std::{fs, process::Command};
use tempfile::TempDir;

fn fixture(delivered: bool, expired: bool) -> (TempDir, Connection, i64) {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("team-compose.yaml"),
        "version: 2.0.1\nbroker: {type: sqlite, path: mail.db}\nprojects: []\n",
    )
    .unwrap();
    let conn = Connection::open(tmp.path().join("mail.db")).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    conn.execute(
        "INSERT INTO approvals (project_id, agent_id, action, summary, status,
            requested_at, expires_at, delivered_at)
         VALUES ('p', 'p:a', 'deploy', 'test', 'pending', 0, ?1, ?2)",
        params![
            if expired { 0.0 } else { 4_000_000_000.0 },
            if delivered { Some(1.0) } else { None }
        ],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    (tmp, conn, id)
}

#[test]
fn expired_cards_cannot_be_decided_and_keep_delivery_semantics() {
    for delivered in [false, true] {
        for command in ["approve", "deny"] {
            let (tmp, conn, id) = fixture(delivered, true);
            let out = Command::new(env!("CARGO_BIN_EXE_teamctl"))
                .arg("--root")
                .arg(tmp.path())
                .args([command, &id.to_string()])
                .output()
                .unwrap();
            assert!(!out.status.success());
            let (status, by, seen): (String, Option<String>, Option<f64>) = conn
                .query_row(
                    "SELECT status, decided_by, delivered_at FROM approvals WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                status,
                if delivered {
                    "expired"
                } else {
                    "undeliverable"
                }
            );
            assert!(by.is_none());
            assert_eq!(seen, if delivered { Some(1.0) } else { None });
        }
    }
}

#[test]
fn successful_decision_is_atomic_and_cannot_be_overwritten() {
    let (tmp, conn, id) = fixture(false, false);
    let decide = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_teamctl"))
            .arg("--root")
            .arg(tmp.path())
            .args([command, &id.to_string(), "--note", "reviewed"])
            .output()
            .unwrap()
    };
    assert!(decide("approve").status.success());
    assert!(!decide("deny").status.success());
    let (status, by, note, seen): (String, String, String, f64) = conn
        .query_row(
            "SELECT status, decided_by, decision_note, delivered_at FROM approvals WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (status.as_str(), by.as_str(), note.as_str()),
        ("approved", "cli", "reviewed")
    );
    assert!(seen > 0.0);
}
