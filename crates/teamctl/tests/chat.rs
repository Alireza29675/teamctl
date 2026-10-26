//! Disposable compose/database and private PTY contracts; no model or messenger calls.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use team_core::chat::{self, ChatMessage, Conversation, InboundMessage, Repository};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    root: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("chat with spaces");
        for path in [
            root.join("projects"),
            root.join("custom"),
            temp.path().join("home"),
            temp.path().join("tmp"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(root.join("team-compose.yaml"), "version: 2.0.0\nbroker: {type: sqlite, path: custom/mail.db}\nprojects: [{file: projects/fixture.yaml}]\n").unwrap();
        fs::write(root.join("projects/fixture.yaml"), "version: 2\nproject: {id: p, name: Fixture, cwd: .}\nmanagers:\n  a: {runtime: codex}\nworkers:\n  b: {runtime: claude, reports_to: a}\n").unwrap();
        let db = root.join("custom/mail.db");
        let conn = chat::open(&db).unwrap();
        conn.execute_batch("INSERT INTO projects(id,name) VALUES('p','Fixture'); INSERT INTO agents(id,project_id,role,runtime) VALUES('p:a','p','manager','fake'),('p:b','p','worker','fake');").unwrap();
        Self { temp, root, db }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamctl"));
        command
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .env("HOME", self.temp.path().join("home"))
            .env("TMPDIR", self.temp.path().join("tmp"))
            .env_remove("TEAMCTL_ROOT")
            .env_remove("TMUX");
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn send(&self, target: &str, text: &str, request: &str) -> ChatMessage {
        serde_json::from_slice(
            &self
                .success(&[
                    "chat",
                    "send",
                    target,
                    text,
                    "--request-id",
                    request,
                    "--json",
                ])
                .stdout,
        )
        .unwrap()
    }

    fn history(&self, target: &str) -> Vec<ChatMessage> {
        serde_json::from_slice(&self.success(&["chat", "history", target, "--json"]).stdout)
            .unwrap()
    }
}

#[test]
fn scriptable_send_shares_local_identity_and_retries_survive_mailbox_gc() {
    let fixture = Fixture::new();
    let sent = fixture.send("a", "سلام 📎 --help", "retry-once");
    let conn = chat::open(&fixture.db).unwrap();
    let repo = Repository::new(&conn);
    let local = repo.open_local("p:a").unwrap();
    assert_eq!(local.id, sent.conversation_id);
    assert_eq!(local.adapter_id, "local");
    assert_eq!(local.peer_id, "operator");
    let reply = repo
        .reply(
            "p:a",
            Some(&local.id),
            None,
            "fake reply",
            "text",
            None,
            None,
        )
        .unwrap();
    conn.execute("DELETE FROM messages", []).unwrap();
    drop(conn);
    let retry = fixture.send(&local.id, "سلام 📎 --help", "retry-once");
    assert_eq!(retry.id, sent.id);
    assert!(retry.mailbox_id.is_none());
    let rows = fixture.history(&local.id);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].text, sent.text);
    assert_eq!(rows[1].id, reply.id);
    let conflict = fixture.output(&[
        "chat",
        "send",
        &local.id,
        "different",
        "--request-id",
        "retry-once",
    ]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("different content"));
    let conversations: Vec<Conversation> =
        serde_json::from_slice(&fixture.success(&["chat", "list", "--json"]).stdout).unwrap();
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].id, local.id);
    assert_eq!(conversations[0].peer_id, "operator");
}

#[test]
fn real_parser_history_pages_and_non_tty_interactive_error() {
    let fixture = Fixture::new();
    let first = fixture.send("p:a", "--literal", "one");
    let second = fixture.send("p:a", "two", "two");
    let third = fixture.send("p:a", "three", "three");
    let before: Vec<ChatMessage> = serde_json::from_slice(
        &fixture
            .success(&[
                "chat",
                "history",
                "p:a",
                "--before",
                &third.id.to_string(),
                "--limit",
                "1",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(before.as_slice(), std::slice::from_ref(&second));
    let after: Vec<ChatMessage> = serde_json::from_slice(
        &fixture
            .success(&[
                "chat",
                "history",
                "p:a",
                "--after",
                &first.id.to_string(),
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(after, [second, third]);
    for args in [
        vec!["chat", "history", "p:a", "--limit", "0"],
        vec!["chat", "history", "p:a", "--before", "2", "--after", "1"],
    ] {
        assert!(!fixture.output(&args).status.success());
    }
    let output = fixture.output(&["chat", "p:a"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive chat needs a terminal"));
    assert!(output.stdout.is_empty());
}

#[test]
fn remote_and_removed_conversations_stay_readable_but_reject_local_sends() {
    let fixture = Fixture::new();
    let local = fixture.send("p:a", "retained", "local");
    let conn = chat::open(&fixture.db).unwrap();
    let remote = Repository::new(&conn)
        .ingest(InboundMessage {
            adapter_id: "telegram:p:b".into(),
            peer_id: "42".into(),
            sender_id: "human".into(),
            agent_id: "p:b".into(),
            external_id: "1".into(),
            text: "remote history".into(),
            kind: "text".into(),
            payload: None,
            title: Some("Messenger".into()),
            reply_to_external_id: None,
        })
        .unwrap();
    let remote_send = fixture.output(&["chat", "send", &remote.conversation_id, "forbidden"]);
    assert!(!remote_send.status.success());
    assert!(String::from_utf8_lossy(&remote_send.stderr)
        .contains("external conversations are read-only"));
    assert_eq!(
        fixture.history(&remote.conversation_id)[0].text,
        "remote history"
    );
    let readable = fixture.success(&["chat", "history", &remote.conversation_id]);
    assert!(String::from_utf8_lossy(&readable.stdout).starts_with("[human ·"));
    conn.execute("DELETE FROM agents WHERE id='p:a'", [])
        .unwrap();
    let removed_send = fixture.output(&["chat", "send", &local.conversation_id, "gone"]);
    assert!(!removed_send.status.success());
    assert!(String::from_utf8_lossy(&removed_send.stderr).contains("unknown agent"));
    assert_eq!(fixture.history(&local.conversation_id)[0].text, "retained");
}

#[cfg(unix)]
mod terminal {
    use super::*;
    use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
    use std::{
        io::{BufRead, BufReader, Read, Write},
        process::{Child, Stdio},
        sync::mpsc::{self, Receiver},
        thread,
        time::{Duration, Instant},
    };

    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn follow_streams_flushed_additions_and_status_changes() {
        let fixture = Fixture::new();
        let first = fixture.send("p:a", "initial", "one");
        let mut child = fixture
            .command(&["chat", "follow", "p:a", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut child = Process(child);
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let row: ChatMessage =
            serde_json::from_str(&receiver.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(row.id, first.id);
        let conn = chat::open(&fixture.db).unwrap();
        let repo = Repository::new(&conn);
        let reply = repo
            .reply(
                "p:a",
                Some(&first.conversation_id),
                None,
                "asynchronous",
                "text",
                None,
                None,
            )
            .unwrap();
        let row: ChatMessage =
            serde_json::from_str(&receiver.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(row.id, reply.id);
        conn.execute("UPDATE chat_messages SET status='failed',error='fixture delivery',revision=revision+1 WHERE id=?1", [reply.id]).unwrap();
        let row: ChatMessage =
            serde_json::from_str(&receiver.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(row.id, reply.id);
        assert_eq!(row.status, chat::DeliveryStatus::Failed);
        assert_eq!(row.error.as_deref(), Some("fixture delivery"));
        assert!(receiver.recv_timeout(Duration::from_millis(500)).is_err());
        unsafe {
            libc::kill(child.0.id() as libc::pid_t, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "follow did not stop on SIGINT");
            thread::sleep(Duration::from_millis(20));
        }
        reader.join().unwrap();
    }

    struct Terminal {
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        writer: Box<dyn Write + Send>,
        receiver: Receiver<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Terminal {
        fn open(fixture: &Fixture, columns: u16) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 20,
                    cols: columns,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_teamctl"));
            command.args(["--root", fixture.root.to_str().unwrap(), "chat", "p:a"]);
            command.env("HOME", fixture.temp.path().join("home"));
            command.env("TMPDIR", fixture.temp.path().join("tmp"));
            command.env("TERM", "xterm-256color");
            command.env_remove("TEAMCTL_ROOT");
            command.env_remove("TMUX");
            let child = pair.slave.spawn_command(command).unwrap();
            drop(pair.slave);
            let writer = pair.master.take_writer().unwrap();
            let mut reader = pair.master.try_clone_reader().unwrap();
            let (sender, receiver) = mpsc::sync_channel(64);
            thread::spawn(move || {
                let mut buffer = [0; 4096];
                while let Ok(count) = reader.read(&mut buffer) {
                    if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            });
            Self {
                master: pair.master,
                child,
                writer,
                receiver,
                output: Vec::new(),
            }
        }

        fn write(&mut self, bytes: &[u8]) {
            self.writer.write_all(bytes).unwrap();
            self.writer.flush().unwrap();
        }

        fn expect(&mut self, fragment: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !String::from_utf8_lossy(&self.output).contains(fragment) {
                assert!(
                    Instant::now() < deadline,
                    "missing {fragment:?}: {}",
                    String::from_utf8_lossy(&self.output)
                );
                if let Ok(bytes) = self.receiver.recv_timeout(Duration::from_millis(50)) {
                    self.output.extend(bytes);
                }
                assert!(self.output.len() < 1024 * 1024, "unbounded terminal output");
            }
        }

        fn close(&mut self) {
            self.write(b"\x04");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                assert!(Instant::now() < deadline, "interactive chat did not exit");
                thread::sleep(Duration::from_millis(20));
            }
            let termios = self.master.get_termios().unwrap();
            assert_ne!(
                termios.local_flags.bits() & libc::ICANON,
                0,
                "raw mode was not restored"
            );
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn private_terminal_keeps_unicode_draft_during_reply_and_sends_once() {
        let fixture = Fixture::new();
        let conn = chat::open(&fixture.db).unwrap();
        let repo = Repository::new(&conn);
        let local = repo.open_local("p:a").unwrap();
        let mut terminal = Terminal::open(&fixture, 40);
        terminal.expect("you> ");
        terminal.write("سلام 📎 qc".as_bytes());
        terminal.expect("you> سلام 📎 qc");
        let reply = repo
            .reply(
                "p:a",
                Some(&local.id),
                None,
                "reply while typing",
                "text",
                None,
                None,
            )
            .unwrap();
        terminal.expect(&format!("[agent · {} · sent] reply while typing", reply.id));
        terminal.output.clear();
        terminal.expect("you> سلام 📎 qc");
        terminal.write(b"\r");
        terminal.expect("[you ·");
        let rows = repo.history(&local.id, None, None, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, reply.id);
        assert_eq!(rows[1].text, "سلام 📎 qc");
        terminal.close();
    }

    #[test]
    fn private_terminal_preserves_failed_draft_and_edit_recovers() {
        let fixture = Fixture::new();
        let conn = chat::open(&fixture.db).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_cli_chat BEFORE INSERT ON chat_messages BEGIN SELECT RAISE(ABORT,'fixture rejected send'); END;").unwrap();
        let mut terminal = Terminal::open(&fixture, 14);
        terminal.expect("you> ");
        terminal.write("界 draft".as_bytes());
        terminal.write(b"\r");
        terminal.expect("send failed:");
        terminal.expect("retry> ");
        let local = Repository::new(&conn).open_local("p:a").unwrap();
        assert!(Repository::new(&conn)
            .history(&local.id, None, None, 10)
            .unwrap()
            .is_empty());
        conn.execute_batch("DROP TRIGGER reject_cli_chat;").unwrap();
        terminal.write(b"!\r");
        terminal.expect("[you ·");
        let rows = Repository::new(&conn)
            .history(&local.id, None, None, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "界 draft!");
        terminal.close();
    }
}
