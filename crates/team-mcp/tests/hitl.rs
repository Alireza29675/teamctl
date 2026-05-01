//! request_approval + org_chart.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

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
    team_core::mailbox::ensure(&conn).unwrap();
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

    // Mark a synthetic delivery before the TTL elapses so the row resolves as
    // `expired` (delivered but no human decision) rather than `undeliverable`.
    let bin = bin();
    let mbx = mailbox.clone();
    let caller = thread::spawn(move || {
        let mut p = Peer::spawn(&bin, "p:mgr", &mbx);
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
        p.shutdown();
        r
    });

    thread::sleep(Duration::from_millis(200));
    let conn = Connection::open(&mailbox).unwrap();
    conn.execute(
        "UPDATE approvals SET delivered_at=strftime('%s','now')
         WHERE delivered_at IS NULL",
        [],
    )
    .unwrap();

    let r = caller.join().unwrap();
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "expired", "got: {r:?}");
}

#[test]
fn request_approval_undeliverable_after_ttl() {
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
                "summary": "no human surface to receive this",
                "ttl_seconds": 1
            }
        }),
    );
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "undeliverable", "got: {r:?}");
    assert!(sc["delivered_at"].is_null(), "got: {r:?}");
    p.shutdown();
}

#[test]
fn request_approval_wait_false_returns_pending_immediately() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:mgr", &mailbox);
    let _ = p.call("initialize", json!({}));
    let start = std::time::Instant::now();
    let r = p.call(
        "tools/call",
        json!({
            "name": "request_approval",
            "arguments": {
                "action": "deploy",
                "summary": "non-blocking diagnostic",
                "ttl_seconds": 60,
                "wait": false
            }
        }),
    );
    let elapsed = start.elapsed();
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "pending", "got: {r:?}");
    assert!(sc["delivered_at"].is_null(), "got: {r:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "wait:false should not block; took {elapsed:?}"
    );
    p.shutdown();
}

// T-038 — Regression-guard for SELECT vs delivered_at flip concurrency.
//
// Today the broker's safety against this race is implicit: SQLite's WAL
// mode serialises writes, the SELECT for pending rows is a snapshot
// read, and the `delivered_at IS NULL` clause makes the flip
// idempotent. The test does not exercise a bug that exists today; it
// exists so that a future refactor toward async/streaming or an
// alternative store can't silently introduce a torn read, a double
// flip, or a lost update without this test failing.
//
// Mechanic: spawn N concurrent SELECT-for-pending threads alongside one
// thread that flips delivered_at on the row, and assert (a) every
// observation of the row sees a coherent state (delivered_at either
// fully NULL or fully set, never a torn intermediate); (b) the row's
// final `delivered_at` is set exactly once (the `WHERE ... IS NULL`
// guard makes the flip idempotent — repeat-flips become no-ops).
#[test]
fn select_vs_delivered_at_flip_is_race_free() {
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    // Seed one pending approval row.
    let conn = Connection::open(&mailbox).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    conn.execute(
        "INSERT INTO approvals (project_id, agent_id, action, summary, status,
                                requested_at, expires_at)
         VALUES ('p', 'mgr', 'publish', 'race test', 'pending', ?1, ?2)",
        params![now, now + 3600.0],
    )
    .unwrap();
    let id: i64 = conn.last_insert_rowid();
    drop(conn);

    let mailbox = Arc::new(mailbox);
    let mut handles = Vec::new();

    // Reader threads: repeatedly SELECT the row's status + delivered_at
    // and assert the pair is coherent. "Coherent" here means: when
    // status is `pending`, delivered_at may be NULL or a real f64 (the
    // flipper races us); never a malformed value or a torn read.
    for _ in 0..4 {
        let mb = Arc::clone(&mailbox);
        handles.push(thread::spawn(move || {
            let conn = Connection::open(&*mb).unwrap();
            conn.busy_timeout(Duration::from_secs(5)).unwrap();
            for _ in 0..200 {
                let (status, delivered_at): (String, Option<f64>) = conn
                    .query_row(
                        "SELECT status, delivered_at FROM approvals WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(status, "pending", "row should remain pending throughout");
                if let Some(t) = delivered_at {
                    assert!(t > 0.0 && t < now + 7200.0, "torn timestamp: {t}");
                }
            }
        }));
    }

    // Flipper thread: hammer the idempotent UPDATE. The first flip wins;
    // subsequent flips become no-ops because of `delivered_at IS NULL`.
    let mb = Arc::clone(&mailbox);
    handles.push(thread::spawn(move || {
        let conn = Connection::open(&*mb).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut total_flipped: usize = 0;
        for _ in 0..50 {
            let n = conn
                .execute(
                    "UPDATE approvals SET delivered_at = ?1
                     WHERE id = ?2 AND delivered_at IS NULL",
                    params![now_secs(), id],
                )
                .unwrap();
            total_flipped += n;
        }
        assert_eq!(
            total_flipped, 1,
            "delivered_at should flip exactly once across 50 attempts"
        );
    }));

    for h in handles {
        h.join().unwrap();
    }

    // Final state: delivered_at set, status still pending.
    let conn = Connection::open(&*mailbox).unwrap();
    let (status, delivered_at): (String, Option<f64>) = conn
        .query_row(
            "SELECT status, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert!(delivered_at.is_some(), "delivered_at should be set");
}
