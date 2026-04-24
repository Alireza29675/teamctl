//! Phase 4: cross-project isolation + bridges.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn seed_two_projects(mailbox: &std::path::Path) {
    let conn = Connection::open(mailbox).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(team_core::mailbox::SCHEMA).unwrap();

    for (pid, pname) in [("alpha", "Alpha"), ("beta", "Beta")] {
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name) VALUES (?1, ?2)",
            params![pid, pname],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager)
             VALUES (?1, ?2, 'manager', 'claude-code', 1)",
            params![format!("{pid}:mgr"), pid],
        )
        .unwrap();
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[test]
fn cross_project_dm_blocked_without_bridge() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed_two_projects(&mailbox);
    let mut a = Peer::spawn(&bin(), "alpha:mgr", &mailbox);
    let _ = a.call("initialize", json!({}));
    let r = a.call(
        "tools/call",
        json!({ "name": "dm", "arguments": { "to": "beta:mgr", "text": "hi" } }),
    );
    assert!(r["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("project isolation"));
    a.shutdown();
}

#[test]
fn bridge_authorizes_cross_project_dm() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed_two_projects(&mailbox);

    // Open a bridge valid for 10 minutes.
    let conn = Connection::open(&mailbox).unwrap();
    conn.execute(
        "INSERT INTO bridges (from_agent, to_agent, topic, opened_by, opened_at, expires_at)
         VALUES ('alpha:mgr','beta:mgr','shared thing','cli',?1,?2)",
        params![now(), now() + 600.0],
    )
    .unwrap();

    let mut a = Peer::spawn(&bin(), "alpha:mgr", &mailbox);
    let _ = a.call("initialize", json!({}));
    let r = a.call(
        "tools/call",
        json!({ "name": "dm", "arguments": { "to": "beta:mgr", "text": "hello" } }),
    );
    assert_eq!(r["result"]["isError"], json!(false), "got: {r:?}");

    // Confirm thread_id was tagged with the bridge id for auditing.
    let thread: Option<String> = conn
        .query_row(
            "SELECT thread_id FROM messages WHERE sender='alpha:mgr' AND recipient='beta:mgr'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        thread.as_deref().unwrap_or("").starts_with("bridge:"),
        "thread_id was {thread:?}"
    );
    a.shutdown();
}

#[test]
fn expired_bridge_rejects_dm() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed_two_projects(&mailbox);

    // Already-expired bridge (expires_at in the past).
    let conn = Connection::open(&mailbox).unwrap();
    conn.execute(
        "INSERT INTO bridges (from_agent, to_agent, topic, opened_by, opened_at, expires_at)
         VALUES ('alpha:mgr','beta:mgr','done','cli',?1,?2)",
        params![now() - 1000.0, now() - 10.0],
    )
    .unwrap();

    let mut a = Peer::spawn(&bin(), "alpha:mgr", &mailbox);
    let _ = a.call("initialize", json!({}));
    let r = a.call(
        "tools/call",
        json!({ "name": "dm", "arguments": { "to": "beta:mgr", "text": "too late" } }),
    );
    assert!(r["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("project isolation"));
    a.shutdown();
}
