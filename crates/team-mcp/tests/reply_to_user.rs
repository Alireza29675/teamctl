//! `reply_to_user` MCP tool: manager-only gate + insert-row semantics.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::tempdir;

struct Peer {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next_id: u64,
}
impl Peer {
    fn spawn(bin: &std::path::Path, agent_id: &str, mailbox: &std::path::Path) -> Self {
        let mut child = Command::new(bin)
            .args([
                "--agent-id",
                agent_id,
                "--mailbox",
                mailbox.to_str().unwrap(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            out,
            next_id: 1,
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let mut buf = String::new();
        self.out.read_line(&mut buf).unwrap();
        serde_json::from_str(&buf).unwrap()
    }
    fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_team-mcp").into()
}

fn seed(mailbox: &std::path::Path) {
    let conn = Connection::open(mailbox).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(team_core::mailbox::SCHEMA).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name) VALUES ('p','P')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:mgr','p','mgr','claude-code',1,NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:dev','p','dev','claude-code',0,'mgr')",
        [],
    )
    .unwrap();
}

#[test]
fn reply_to_user_inserts_row_when_caller_is_manager() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:mgr", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({
            "name": "reply_to_user",
            "arguments": { "text": "hello human" }
        }),
    );
    p.shutdown();

    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["recipient"], "user:telegram");
    assert!(sc["id"].is_number());

    // Verify the message landed in the mailbox with the right shape.
    let conn = Connection::open(&mailbox).unwrap();
    let (sender, recipient, text, project): (String, String, String, String) = conn
        .query_row(
            "SELECT sender, recipient, text, project_id FROM messages
             WHERE id = ?1",
            rusqlite::params![sc["id"].as_i64().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(sender, "p:mgr");
    assert_eq!(recipient, "user:telegram");
    assert_eq!(text, "hello human");
    assert_eq!(project, "p");
}

#[test]
fn reply_to_user_rejects_non_manager_caller() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:dev", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({
            "name": "reply_to_user",
            "arguments": { "text": "hello from dev" }
        }),
    );
    p.shutdown();

    // The MCP server returns an error result; check that we did NOT
    // get a structuredContent payload back.
    assert!(
        r["error"].is_object() || r["result"]["isError"].as_bool() == Some(true),
        "expected an error response for non-manager reply_to_user, got {r}"
    );

    // And no message row should exist.
    let conn = Connection::open(&mailbox).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "non-manager reply_to_user must not insert a row");
}
