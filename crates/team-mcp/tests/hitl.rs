//! request_approval + org_chart.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::Duration;

use rusqlite::{params, Connection};
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
fn org_chart_returns_managers_and_workers() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);
    let mut p = Peer::spawn(&bin(), "p:dev", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({ "name": "org_chart", "arguments": {} }),
    );
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["project"], "p");
    assert_eq!(sc["managers"][0]["id"], "p:mgr");
    assert_eq!(sc["workers"][0]["id"], "p:dev");
    assert_eq!(sc["workers"][0]["reports_to"], "mgr");
    p.shutdown();
}

#[test]
fn request_approval_blocks_then_resolves_on_approve() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let bin = bin();
    let mbx = mailbox.clone();
    // Spawn the caller in a thread so we can approve it from the main thread.
    let caller = thread::spawn(move || {
        let mut p = Peer::spawn(&bin, "p:mgr", &mbx);
        let _ = p.call("initialize", json!({}));
        let r = p.call(
            "tools/call",
            json!({
                "name": "request_approval",
                "arguments": {
                    "action": "publish",
                    "summary": "post to r/vancouver",
                    "ttl_seconds": 30
                }
            }),
        );
        p.shutdown();
        r
    });

    // Give the caller a moment to insert its pending row.
    thread::sleep(Duration::from_millis(400));
    let conn = Connection::open(&mailbox).unwrap();
    let id: i64 = conn
        .query_row(
            "SELECT id FROM approvals WHERE status='pending' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE approvals SET status='approved', decided_at=strftime('%s','now'), decided_by='test'
         WHERE id=?1",
        params![id],
    )
    .unwrap();

    let r = caller.join().unwrap();
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "approved");
    assert_eq!(sc["id"], id);
}

#[test]
fn request_approval_expires_after_ttl() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:mgr", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({
            "name": "request_approval",
            "arguments": {
                "action": "deploy",
                "summary": "risky deploy",
                "ttl_seconds": 1
            }
        }),
    );
    // Note: ttl=1 clamps to the schema min of 30; the request_approval path
    // honors the literal value. Sleep 2s on top of the long-poll budget is
    // not needed — the poll loop itself waits out the ttl.
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "expired", "got: {r:?}");
    p.shutdown();
}
