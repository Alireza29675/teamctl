//! Channels delivery: when a new inbox row arrives, the team-mcp child
//! pushes an unsolicited `notifications/claude/channel` JSON-RPC frame
//! on stdout (per Claude Code's Channels wire format).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::tempdir;

fn team_mcp_bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_team-mcp").into()
}

/// Reads stdout on a worker thread and surfaces lines via mpsc, so callers
/// can `recv_timeout` instead of relying on a `read()` that blocks past
/// our deadline (the negative-path test would otherwise wait for the
/// child to close stdout).
struct Lines {
    rx: Receiver<String>,
}

impl Lines {
    fn spawn(out: ChildStdout) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut r = BufReader::new(out);
            loop {
                let mut buf = String::new();
                match r.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(buf).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self { rx }
    }

    fn recv_json(&self, budget: Duration) -> Value {
        let line = self
            .rx
            .recv_timeout(budget)
            .expect("expected JSON line from child");
        serde_json::from_str(&line).expect("valid json")
    }

    /// Drain frames until one with `method == m` arrives, or `budget`
    /// elapses. Non-matching frames (e.g. responses) are discarded.
    fn wait_for_method(&self, m: &str, budget: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if v["method"] == m {
                            return Some(v);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

struct Peer {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Lines,
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
        let lines = Lines::spawn(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn write(&mut self, v: &Value) {
        let mut s = serde_json::to_string(v).unwrap();
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

#[test]
fn initialize_advertises_channel_capability() {
    // Without `experimental.claude/channel: {}` Claude Code does not
    // register a listener and silently drops every channel notification
    // we emit — which is exactly the regression that landed in 0.6.x.
    // `serverInfo.name` becomes the `<channel source="...">` attribute,
    // and the bootstrap prompt + .mcp.json key both read "team".
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let mut p = Peer::spawn(&team_mcp_bin(), "hello:dev", &mailbox);
    p.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let resp = p.lines.recv_json(Duration::from_secs(2));
    assert_eq!(
        resp["result"]["capabilities"]["experimental"]["claude/channel"],
        json!({}),
        "initialize must advertise the claude/channel capability; got {resp}"
    );
    assert_eq!(resp["result"]["serverInfo"]["name"], "team");
    p.shutdown();
}

#[test]
fn new_inbox_row_pushes_channel_notification_to_subscribed_agent() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    // dev: the receiver. Initialise + signal `notifications/initialized`
    // so its channel watcher unblocks.
    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(Duration::from_secs(2));
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    // mgr: the sender. Does NOT trip its initialised gate, so its watcher
    // stays parked and won't pollute mgr's stdout — keeps "one response
    // per call" simple on this side.
    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(Duration::from_secs(2));

    // Give dev's watcher a beat to capture the (empty) high-water mark
    // before the row arrives, so the new row strictly exceeds last_seen.
    thread::sleep(Duration::from_millis(150));

    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "dm",
            "arguments": { "to": "dev", "text": "ping via channels" }
        }
    }));
    let dm_resp = mgr.lines.recv_json(Duration::from_secs(2));
    let msg_id = dm_resp["result"]["structuredContent"]["id"]
        .as_i64()
        .expect("dm returned no id");

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", Duration::from_secs(5))
        .expect("expected notifications/claude/channel within 5s");

    // Per the Channels wire format, `params.meta` is `Record<string, string>`.
    // Numbers / nulls cause Claude Code to silently drop the notification, so
    // every value must be a string and absent fields must be omitted (not null).
    assert_eq!(notif["params"]["content"], "ping via channels");
    let meta = &notif["params"]["meta"];
    assert_eq!(meta["sender"], "hello:mgr");
    assert_eq!(meta["recipient"], "hello:dev");
    assert_eq!(meta["id"], msg_id.to_string());
    assert!(meta["sent_at"].is_string(), "sent_at must be a string");
    assert!(
        meta.get("thread_id").is_none() || meta["thread_id"].is_string(),
        "thread_id must be absent or a string, never null"
    );
    for (k, v) in meta.as_object().expect("meta is an object") {
        assert!(v.is_string(), "meta.{k} must be a string, got {v}");
    }

    dev.shutdown();
    mgr.shutdown();
}

#[test]
fn watcher_skips_pre_existing_unacked_messages_at_startup() {
    // Pre-existing unacked mail must be left for the agent to fetch via
    // `inbox_peek` — pushing it as a channel event would race the agent's
    // own catch-up call and double-deliver. Assert silence on stdout for
    // a generous window after the gate trips.
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    // Seed an inbox row for dev *before* dev's process starts.
    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(Duration::from_secs(2));
    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "dm",
            "arguments": { "to": "dev", "text": "stale, not via channels" }
        }
    }));
    let _ = mgr.lines.recv_json(Duration::from_secs(2));
    mgr.shutdown();

    // Now spawn dev and trip the initialised gate.
    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(Duration::from_secs(2));
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    // 1.5 s — three watcher ticks. No notification should arrive.
    let leaked = dev
        .lines
        .wait_for_method("notifications/claude/channel", Duration::from_millis(1500));
    assert!(
        leaked.is_none(),
        "watcher leaked pre-existing message as a channel event: {leaked:?}"
    );

    dev.shutdown();
}
