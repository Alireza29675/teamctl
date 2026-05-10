//! `compact_self` MCP tool: manager + claude-code gates and the
//! fire-and-forget dispatch shape. The actual tmux side effect is not
//! exercised here — the handler returns immediately and runs the
//! send-keys on the blocking pool, so the test only asserts on the MCP
//! response shape and the absence of a stored mailbox row.

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
                "--tmux-prefix",
                "t-",
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
    // codex-runtime manager — used to pin the runtime gate end-to-end.
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:codex','p','codex','codex',1,NULL)",
        [],
    )
    .unwrap();
}

#[test]
fn compact_self_listed_in_tools_list() {
    // Surface check: any change to the schema array that drops
    // `compact_self` would silently make the tool invisible to clients.
    // Pin its presence at the wire boundary.
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:mgr", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call("tools/list", json!({}));
    p.shutdown();

    let tools = r["result"]["tools"].as_array().unwrap();
    let found = tools.iter().any(|t| t["name"] == "compact_self");
    assert!(found, "compact_self must appear in tools/list: {r}");
}

#[test]
fn compact_self_dispatch_returns_session_for_claude_code_manager() {
    // Happy path through the wire: claude-code manager call resolves
    // to the canonical session name and returns "dispatched". The tmux
    // side effect runs on the blocking pool and isn't observed here.
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:mgr", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({ "name": "compact_self", "arguments": {} }),
    );
    p.shutdown();

    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "dispatched");
    assert_eq!(sc["session"], "t-p-mgr");

    // Fire-and-forget contract: no mailbox row is created. compact_self
    // doesn't ride the kind/structured_payload discriminator the way
    // reply_to_user / show_typing / react_to_user do — it goes straight
    // to tmux.
    let conn = Connection::open(&mailbox).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "compact_self must not insert a mailbox row");
}

#[test]
fn compact_self_dispatch_works_for_worker_too() {
    // No manager gate at the wire: a claude-code worker calling
    // compact_self lands the same dispatched response as a manager,
    // routed to its own tmux pane. Role instructions decide when to
    // call; the destructive warning in the schema carries the safety.
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:dev", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({ "name": "compact_self", "arguments": {} }),
    );
    p.shutdown();

    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["status"], "dispatched");
    assert_eq!(sc["session"], "t-p-dev");
}

#[test]
fn compact_self_rejects_non_claude_code_manager() {
    // Runtime gate at the wire: a manager on a non-claude-code runtime
    // (codex here) is rejected. /compact is Claude-Code-only; rather
    // than fire send-keys blindly we surface a clean MCP error.
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);

    let mut p = Peer::spawn(&bin(), "p:codex", &mailbox);
    let _ = p.call("initialize", json!({}));
    let r = p.call(
        "tools/call",
        json!({ "name": "compact_self", "arguments": {} }),
    );
    p.shutdown();

    assert!(
        r["error"].is_object() || r["result"]["isError"].as_bool() == Some(true),
        "expected an error response for non-claude-code compact_self, got {r}"
    );
}
