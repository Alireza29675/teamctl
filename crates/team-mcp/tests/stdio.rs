//! End-to-end smoke: drive a real `team-mcp` process over stdio.
//!
//! Spawns two agents in the same project, has one DM the other, then
//! exercises `inbox_peek` and `inbox_ack`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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
            .expect("spawn team-mcp");
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
        if let Err(e) = self.stdin.write_all(line.as_bytes()) {
            panic!(
                "write {method}: {e}; child exited={:?}",
                self.child.try_wait()
            );
        }
        self.stdin.flush().unwrap();
        let mut buf = String::new();
        self.out.read_line(&mut buf).expect("response line");
        serde_json::from_str(&buf).expect("valid json response")
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn team_mcp_bin() -> std::path::PathBuf {
    // Target path env that `cargo test` provides for binaries in the same package.
    env!("CARGO_BIN_EXE_team-mcp").into()
}

#[test]
fn end_to_end_dm_between_two_agents() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);

    // Both sides initialize.
    let _ = mgr.call("initialize", json!({}));
    let _ = dev.call("initialize", json!({}));

    // Manager DMs dev (bare name resolves to same project).
    let send = mgr.call(
        "tools/call",
        json!({ "name": "dm", "arguments": { "to": "dev", "text": "hello, world" } }),
    );
    assert!(send["result"]["isError"] == json!(false));
    let msg_id = send["result"]["structuredContent"]["id"].as_i64().unwrap();

    // Dev peeks and sees it.
    let peek = dev.call(
        "tools/call",
        json!({ "name": "inbox_peek", "arguments": {} }),
    );
    let msgs = peek["result"]["structuredContent"]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "hello, world");
    assert_eq!(msgs[0]["sender"], "hello:mgr");
    assert_eq!(msgs[0]["id"], msg_id);

    // Ack and re-peek — empty.
    let ack = dev.call(
        "tools/call",
        json!({ "name": "inbox_ack", "arguments": { "ids": [msg_id] } }),
    );
    assert_eq!(ack["result"]["structuredContent"]["acked"], 1);
    let peek2 = dev.call(
        "tools/call",
        json!({ "name": "inbox_peek", "arguments": {} }),
    );
    assert_eq!(
        peek2["result"]["structuredContent"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    mgr.shutdown();
    dev.shutdown();
}

#[test]
fn cross_project_dm_is_rejected() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut a = Peer::spawn(&bin, "alpha:mgr", &mailbox);
    let _ = a.call("initialize", json!({}));

    let send = a.call(
        "tools/call",
        json!({
            "name": "dm",
            "arguments": { "to": "beta:mgr", "text": "hi" }
        }),
    );
    // The dispatcher maps our Err into a JSON-RPC error.
    assert!(send["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("project isolation"));
    a.shutdown();
}
