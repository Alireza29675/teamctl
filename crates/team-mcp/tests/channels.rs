//! ACLs + broadcast + channel delivery.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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

/// Bootstrap the channel + ACL tables by hand (test doesn't run `teamctl up`).
fn seed(mailbox: &std::path::Path) {
    let conn = Connection::open(mailbox).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(team_core::mailbox::SCHEMA).unwrap();

    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name) VALUES ('p', 'P')",
        [],
    )
    .unwrap();
    for (a, mgr) in [("p:mgr", 1), ("p:dev1", 0), ("p:dev2", 0), ("p:critic", 0)] {
        conn.execute(
            "INSERT INTO agents (id, project_id, role, runtime, is_manager) VALUES (?1,'p',?2,'claude-code',?3)
             ON CONFLICT(id) DO UPDATE SET is_manager=excluded.is_manager",
            params![a, a.split_once(':').unwrap().1, mgr],
        ).unwrap();
    }
    // ACLs
    let set_acl = |id: &str, dm: &[&str], bc: &[&str]| {
        conn.execute(
            "INSERT INTO agent_acls (agent_id, can_dm_json, can_bcast_json)
             VALUES (?1,?2,?3) ON CONFLICT(agent_id) DO UPDATE SET can_dm_json=excluded.can_dm_json, can_bcast_json=excluded.can_bcast_json",
            params![id, serde_json::to_string(dm).unwrap(), serde_json::to_string(bc).unwrap()],
        ).unwrap();
    };
    set_acl("p:mgr", &["dev1", "dev2", "critic"], &["product", "all"]);
    set_acl("p:dev1", &["mgr", "dev2"], &["product", "internal"]);
    set_acl("p:dev2", &["mgr", "dev1"], &["product", "internal"]);
    set_acl("p:critic", &["mgr"], &["product"]);
    // Channels
    for (name, explicit) in [
        ("product", vec!["p:mgr", "p:dev1", "p:dev2", "p:critic"]),
        ("internal", vec!["p:dev1", "p:dev2"]),
        ("all", vec!["p:mgr", "p:dev1", "p:dev2", "p:critic"]),
    ] {
        let cid = format!("p:{name}");
        conn.execute(
            "INSERT INTO channels (id, project_id, name, wildcard) VALUES (?1,'p',?2,?3)",
            params![cid, name, if name == "all" { 1 } else { 0 }],
        )
        .unwrap();
        for a in explicit {
            conn.execute(
                "INSERT INTO channel_members (channel_id, agent_id) VALUES (?1, ?2)",
                params![cid, a],
            )
            .unwrap();
        }
    }
}

#[test]
fn broadcast_delivers_to_subscribers_only() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);
    let bin = bin();

    let mut dev1 = Peer::spawn(&bin, "p:dev1", &mailbox);
    let mut dev2 = Peer::spawn(&bin, "p:dev2", &mailbox);
    let mut mgr = Peer::spawn(&bin, "p:mgr", &mailbox);
    let _ = dev1.call("initialize", json!({}));
    let _ = dev2.call("initialize", json!({}));
    let _ = mgr.call("initialize", json!({}));

    // dev1 broadcasts to #internal. dev2 should see it, mgr should not.
    let r = dev1.call(
        "tools/call",
        json!({ "name": "broadcast", "arguments": { "channel": "internal", "text": "dev-only note" } }),
    );
    assert_eq!(r["result"]["isError"], json!(false), "err: {r:?}");

    let dev2_peek = dev2.call(
        "tools/call",
        json!({ "name": "inbox_peek", "arguments": {} }),
    );
    let msgs = dev2_peek["result"]["structuredContent"]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "dev-only note");

    let mgr_peek = mgr.call(
        "tools/call",
        json!({ "name": "inbox_peek", "arguments": {} }),
    );
    assert_eq!(
        mgr_peek["result"]["structuredContent"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "manager should not see #internal traffic"
    );

    dev1.shutdown();
    dev2.shutdown();
    mgr.shutdown();
}

#[test]
fn broadcast_rejected_when_not_member() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);
    let bin = bin();

    let mut critic = Peer::spawn(&bin, "p:critic", &mailbox);
    let _ = critic.call("initialize", json!({}));

    let r = critic.call(
        "tools/call",
        json!({ "name": "broadcast", "arguments": { "channel": "internal", "text": "sneak" } }),
    );
    assert!(r["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not a member"));
    critic.shutdown();
}

#[test]
fn dm_rejected_when_acl_denies() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("m.db");
    seed(&mailbox);
    let bin = bin();

    // critic can only DM `mgr`; try to DM `dev1`.
    let mut critic = Peer::spawn(&bin, "p:critic", &mailbox);
    let _ = critic.call("initialize", json!({}));
    let r = critic.call(
        "tools/call",
        json!({ "name": "dm", "arguments": { "to": "dev1", "text": "hi" } }),
    );
    assert!(r["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not permitted to DM"));
    critic.shutdown();
}
