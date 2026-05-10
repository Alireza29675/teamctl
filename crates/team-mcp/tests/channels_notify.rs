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

/// Budget for an expected JSON-RPC frame from the child — both direct
/// responses (`recv_json`) and unsolicited channel notifications
/// (`wait_for_method`). Hot-path wall clock is well under 100ms, and
/// these tests don't rely on tight timing for any signal — they're
/// positive-path correctness checks. The budget exists only so a hung
/// child doesn't pin CI forever.
///
/// Sized for the worst CI runner we observe, not the typical one:
/// `cargo test` runs this file's tests in parallel, each spawning 1–2
/// team-mcp child processes, and the GitHub-hosted ubuntu-24.04
/// stable runner has been measured paying >10s for the first frame
/// under load (qa bounce on PR #119 hit 10.18s on the prior 10s
/// budget). 30s gives generous headroom against that worst case
/// while still surfacing genuine hangs in well under a minute.
///
/// The negative-path test
/// (`watcher_skips_pre_existing_unacked_messages_at_startup`) keeps
/// its own deliberately-short 1500ms budget — we want it to fail
/// fast when no notification is expected.
const RPC_BUDGET: Duration = Duration::from_secs(30);

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

    /// Drain frames until a JSON-RPC response with `id == want` arrives.
    /// Skips notifications / unrelated responses. Panics on timeout — used
    /// in tests where the response is expected.
    fn wait_for_response(&self, want: i64, budget: Duration) -> Value {
        let deadline = std::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for response id={want}"
            );
            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if v.get("id").and_then(|i| i.as_i64()) == Some(want) {
                            return v;
                        }
                    }
                }
                Err(_) => panic!("disconnected waiting for response id={want}"),
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
        Self::spawn_with_env(bin, agent_id, mailbox, &[])
    }

    fn spawn_with_env(
        bin: &std::path::Path,
        agent_id: &str,
        mailbox: &std::path::Path,
        env: &[(&str, &str)],
    ) -> Self {
        let mut cmd = Command::new(bin);
        cmd.args([
            "--agent-id",
            agent_id,
            "--mailbox",
            mailbox.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        // T-104: tests that exercise the lazy-inbox flag plumb env vars
        // through here. Default empty = inherit from runner.
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn team-mcp");
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
    let resp = p.lines.recv_json(RPC_BUDGET);
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
    let _ = dev.lines.recv_json(RPC_BUDGET);
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    // mgr: the sender. Does NOT trip its initialised gate, so its watcher
    // stays parked and won't pollute mgr's stdout — keeps "one response
    // per call" simple on this side.
    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(RPC_BUDGET);

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
    let dm_resp = mgr.lines.recv_json(RPC_BUDGET);
    let msg_id = dm_resp["result"]["structuredContent"]["id"]
        .as_i64()
        .expect("dm returned no id");

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("expected notifications/claude/channel");

    // T-104: lazy inbox is the default — the wire `content` is a short
    // stub and `meta.lazy` is set so the agent knows to drill in via
    // `inbox_read`. The full body stays in the store; the stub carries
    // only sender + an 80-char preview.
    assert_eq!(
        notif["params"]["content"],
        "📬 1 new message from hello:mgr: \"ping via channels\"",
    );
    let meta = &notif["params"]["meta"];
    assert_eq!(meta["sender"], "hello:mgr");
    assert_eq!(meta["recipient"], "hello:dev");
    assert_eq!(meta["id"], msg_id.to_string());
    assert_eq!(meta["lazy"], "1", "lazy stub must mark itself with lazy=1");
    assert!(meta["sent_at"].is_string(), "sent_at must be a string");
    assert!(
        meta.get("thread_id").is_none() || meta["thread_id"].is_string(),
        "thread_id must be absent or a string, never null"
    );
    // Per the Channels wire format, `params.meta` is `Record<string, string>`.
    // Numbers / nulls cause Claude Code to silently drop the notification, so
    // every value must be a string and absent fields must be omitted (not null).
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
    let _ = mgr.lines.recv_json(RPC_BUDGET);
    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "dm",
            "arguments": { "to": "dev", "text": "stale, not via channels" }
        }
    }));
    let _ = mgr.lines.recv_json(RPC_BUDGET);
    mgr.shutdown();

    // Now spawn dev and trip the initialised gate.
    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);
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

/// Deflake regression: a row inserted *between* `team-mcp` spawning
/// and the client sending `notifications/initialized` must still fire
/// a channel notification once initialized lands. Before the fix at
/// `main.rs::spawn_channel_watcher`, the watcher computed its
/// high-water mark inside the spawned task — *after* awaiting on
/// `initialized.notified()` — so any row written in that window was
/// captured in `last_seen` and silently never emitted. The
/// `immediate_message_delivers_full_body_inline` flake was the
/// canary; this test pins the broader contract so a future refactor
/// that moves the high-water mark back inside the task fails loudly.
#[test]
fn row_written_before_initialized_still_emits_notification() {
    use rusqlite::Connection;

    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    // Pre-create the schema so the post-spawn INSERT below doesn't
    // hit a missing-table error. No row is inserted yet — the race
    // window we exercise is "row arrives after spawn but before
    // `notifications/initialized`", written further down.
    let conn = Connection::open(&mailbox).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    drop(conn);

    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);

    // Race window: row inserted after spawn but before initialized.
    // With `last_seen` captured synchronously at spawn, this row's
    // id > last_seen, so it gets emitted on the watcher's first poll
    // after initialized lands. The pre-fix code would have captured
    // this id in last_seen and never emitted.
    let conn = Connection::open(&mailbox).unwrap();
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, delivery_mode)
         VALUES ('hello', 'user:telegram', 'hello:dev',
                 'arrived during the boot window', strftime('%s','now'),
                 'immediate')",
        [],
    )
    .unwrap();
    drop(conn);

    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("row inserted post-spawn must emit even when written before initialized");
    assert_eq!(
        notif["params"]["content"], "arrived during the boot window",
        "the boot-window row must surface as a channel notification"
    );

    dev.shutdown();
}

/// T-104: a row inserted with `delivery_mode='immediate'` (the path the
/// bot takes when an operator prefixes the message with `/readnow `)
/// must land inline as the full body, with no `meta.lazy` flag.
#[test]
fn immediate_message_delivers_full_body_inline() {
    use rusqlite::Connection;

    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    thread::sleep(Duration::from_millis(150));

    // Write directly with `delivery_mode='immediate'` to mimic the
    // bot's `/readnow` path without dragging the bot crate into this test.
    let conn = Connection::open(&mailbox).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    conn.execute(
        "INSERT INTO messages
            (project_id, sender, recipient, text, sent_at, delivery_mode)
         VALUES ('hello', 'user:telegram', 'hello:dev',
                 'the build just broke, can you investigate', strftime('%s','now'),
                 'immediate')",
        [],
    )
    .unwrap();
    drop(conn);

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("expected notifications/claude/channel");
    assert_eq!(
        notif["params"]["content"], "the build just broke, can you investigate",
        "immediate rows must deliver the full body inline"
    );
    assert!(
        notif["params"]["meta"].get("lazy").is_none(),
        "immediate rows must not carry the lazy stub flag"
    );

    dev.shutdown();
}

/// T-104: `TEAM_LAZY_INBOX=0` is the global escape hatch. With it set,
/// every row delivers full-body, regardless of `delivery_mode`.
#[test]
fn lazy_inbox_disabled_by_env_delivers_full_body() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut dev = Peer::spawn_with_env(&bin, "hello:dev", &mailbox, &[("TEAM_LAZY_INBOX", "0")]);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(RPC_BUDGET);
    thread::sleep(Duration::from_millis(150));

    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "dm",
            "arguments": { "to": "dev", "text": "full inline because env opted out" }
        }
    }));
    let _ = mgr.lines.recv_json(RPC_BUDGET);

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("expected notifications/claude/channel");
    assert_eq!(
        notif["params"]["content"], "full inline because env opted out",
        "TEAM_LAZY_INBOX=0 must restore full-body inline delivery"
    );
    assert!(
        notif["params"]["meta"].get("lazy").is_none(),
        "with lazy disabled, no lazy=1 marker"
    );

    dev.shutdown();
    mgr.shutdown();
}

/// T-104: stubs for messages addressed to a channel show the channel
/// name so the agent can tell DM vs channel context at a glance.
#[test]
fn channel_stub_includes_channel_name_in_preview() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    // Seed schema + a channel `hello:dev` with `hello:dev-agent` as a member.
    let conn = rusqlite::Connection::open(&mailbox).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name) VALUES ('hello', 'hello')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agents (id, project_id, role, runtime, is_manager)
         VALUES ('hello:dev-agent', 'hello', 'dev', 'claude', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channels (id, project_id, name, wildcard) VALUES ('hello:dev', 'hello', 'dev', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channel_members (channel_id, agent_id) VALUES ('hello:dev', 'hello:dev-agent')",
        [],
    )
    .unwrap();
    drop(conn);

    let mut dev = Peer::spawn(&bin, "hello:dev-agent", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    thread::sleep(Duration::from_millis(150));

    let conn = rusqlite::Connection::open(&mailbox).unwrap();
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
         VALUES ('hello', 'hello:mgr', 'channel:hello:dev', 'standup in 5', strftime('%s','now'))",
        [],
    )
    .unwrap();
    drop(conn);

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("expected notifications/claude/channel");
    assert_eq!(
        notif["params"]["content"],
        "📬 1 new message in dev from hello:mgr: \"standup in 5\"",
    );
    assert_eq!(notif["params"]["meta"]["lazy"], "1");

    dev.shutdown();
}

/// T-104: bodies longer than the 80-char preview window get truncated
/// with a trailing ellipsis. The full body remains intact in the store
/// (verified via `inbox_read`).
#[test]
fn long_body_preview_truncates_at_80_with_ellipsis() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);
    dev.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(RPC_BUDGET);
    thread::sleep(Duration::from_millis(150));

    let body = "x".repeat(200);
    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "dm",
            "arguments": { "to": "dev", "text": body }
        }
    }));
    let _ = mgr.lines.recv_json(RPC_BUDGET);

    let notif = dev
        .lines
        .wait_for_method("notifications/claude/channel", RPC_BUDGET)
        .expect("expected notifications/claude/channel");
    let content = notif["params"]["content"].as_str().unwrap().to_string();
    assert!(
        content.contains(&"x".repeat(80)),
        "stub must include the 80-char preview prefix: {content}"
    );
    assert!(
        content.ends_with("…\""),
        "stub must end with ellipsis-quote: {content}"
    );
    assert!(
        !content.contains(&"x".repeat(81)),
        "stub must NOT contain 81 copies — preview is 80-char hard cap: {content}",
    );

    dev.shutdown();
    mgr.shutdown();
}

/// T-104: `inbox_read` returns full bodies AND auto-acks. Subsequent
/// `inbox_peek` calls must not see those rows again.
#[test]
fn inbox_read_returns_bodies_and_auto_acks() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);

    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(RPC_BUDGET);

    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "dm", "arguments": { "to": "dev", "text": "first" } }
    }));
    let dm1 = mgr.lines.recv_json(RPC_BUDGET);
    let id1 = dm1["result"]["structuredContent"]["id"].as_i64().unwrap();

    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "dm", "arguments": { "to": "dev", "text": "second" } }
    }));
    let dm2 = mgr.lines.recv_json(RPC_BUDGET);
    let id2 = dm2["result"]["structuredContent"]["id"].as_i64().unwrap();

    // Read both — should get full bodies back AND ack them.
    dev.write(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": { "name": "inbox_read", "arguments": { "ids": [id1, id2] } }
    }));
    let read_resp = dev.lines.wait_for_response(10, RPC_BUDGET);
    let msgs = read_resp["result"]["structuredContent"]["messages"]
        .as_array()
        .expect("inbox_read returns messages array");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["text"], "first");
    assert_eq!(msgs[1]["text"], "second");

    // Now peek — no remaining unacked rows.
    dev.write(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": { "name": "inbox_peek", "arguments": {} }
    }));
    let peek_resp = dev.lines.wait_for_response(11, RPC_BUDGET);
    let remaining = peek_resp["result"]["structuredContent"]["messages"]
        .as_array()
        .expect("inbox_peek returns messages array");
    assert!(
        remaining.is_empty(),
        "inbox_read must auto-ack — peek should be empty, got {remaining:?}"
    );

    dev.shutdown();
    mgr.shutdown();
}

/// T-104: `inbox_read` must not let one agent fetch + ack messages
/// addressed to another agent. Same-process callers always pass through
/// the ACL filter in the SQL.
#[test]
fn inbox_read_rejects_ids_addressed_to_other_agents() {
    let tmp = tempdir().unwrap();
    let mailbox = tmp.path().join("mailbox.db");
    let bin = team_mcp_bin();

    let mut dev = Peer::spawn(&bin, "hello:dev", &mailbox);
    dev.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = dev.lines.recv_json(RPC_BUDGET);

    let mut mgr = Peer::spawn(&bin, "hello:mgr", &mailbox);
    mgr.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = mgr.lines.recv_json(RPC_BUDGET);

    let mut other = Peer::spawn(&bin, "hello:other", &mailbox);
    other.write(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let _ = other.lines.recv_json(RPC_BUDGET);

    // mgr DMs `other`, not `dev`.
    mgr.write(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "dm", "arguments": { "to": "other", "text": "private to other" } }
    }));
    let dm_resp = mgr.lines.recv_json(RPC_BUDGET);
    let id_for_other = dm_resp["result"]["structuredContent"]["id"]
        .as_i64()
        .unwrap();

    // dev tries to read it — must come back empty, and the row must
    // remain unacked (`other` can still see it).
    dev.write(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": { "name": "inbox_read", "arguments": { "ids": [id_for_other] } }
    }));
    let read_resp = dev.lines.wait_for_response(10, RPC_BUDGET);
    let msgs = read_resp["result"]["structuredContent"]["messages"]
        .as_array()
        .unwrap();
    assert!(
        msgs.is_empty(),
        "dev must not fetch messages addressed to other: {msgs:?}"
    );

    other.write(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": { "name": "inbox_peek", "arguments": {} }
    }));
    let peek_resp = other.lines.wait_for_response(11, RPC_BUDGET);
    let remaining = peek_resp["result"]["structuredContent"]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "row must remain visible to its true recipient: {remaining:?}"
    );

    dev.shutdown();
    mgr.shutdown();
    other.shutdown();
}
