use super::fixture::{wait, Fixture};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use team_core::chat::{self, InboundMessage, Repository};

#[test]
fn tty_identify_prints_only_ids_drops_inspection_and_never_sends_queued_work() {
    let f = Fixture::new("normal");
    let conn = f.database();
    let repo = Repository::new(&conn);
    let original = repo
        .ingest(InboundMessage {
            adapter_id: "bridge".into(),
            peer_id: "peer:one".into(),
            sender_id: "human:one".into(),
            agent_id: "p:manager".into(),
            external_id: "existing:1".into(),
            text: "history".into(),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap();
    let queued = repo
        .reply(
            "p:manager",
            Some(&original.conversation_id),
            None,
            "DO_NOT_SEND",
            "text",
            None,
            None,
        )
        .unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 90,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_teamctl"));
    command.args([
        "--root",
        f.root.to_str().unwrap(),
        "adapter",
        "identify",
        "bridge",
    ]);
    command.env("HOME", &f.home);
    command.env("XDG_STATE_HOME", &f.state);
    command.env("PATH", &f.path);
    command.env("FIXTURE_SOURCE", "first line\nsecond line '$()'");
    command.env("TERM", "xterm-256color");
    command.env_remove("TEAMCTL_ROOT");
    command.env_remove("TMUX");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let collected = output.clone();
    let reading = thread::spawn(move || {
        let mut bytes = [0; 4096];
        while let Ok(count) = reader.read(&mut bytes) {
            if count == 0 {
                break;
            }
            collected.lock().unwrap().extend_from_slice(&bytes[..count]);
        }
    });
    wait(|| {
        String::from_utf8_lossy(&output.lock().unwrap()).contains("sender_id=\"human:forbidden\"")
    });
    assert!(!String::from_utf8_lossy(&output.lock().unwrap()).contains("DO_NOT_DISPLAY"));
    let competing = f.command(&["adapter", "run", "bridge"]).output().unwrap();
    assert!(!competing.status.success());
    f.cli(&["adapter", "stop", "bridge"]);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(25));
    }
    drop(pair.master);
    reading.join().unwrap();
    assert_eq!(
        repo.history(&original.conversation_id, None, None, 20)
            .unwrap()
            .len(),
        2
    );
    assert!(repo
        .history(&original.conversation_id, None, None, 20)
        .unwrap()
        .iter()
        .any(|m| m.id == queued.id && m.status == chat::DeliveryStatus::Queued));
    assert!(!f
        .events("bridge")
        .iter()
        .any(|e| e["frame"]["type"] == "send"));
    assert_eq!(
        f.events("bridge")
            .iter()
            .filter(|e| e["frame"]["type"] == "accepted")
            .count(),
        3
    );
}

#[test]
fn terminal_eof_and_ctrl_c_close_the_owned_transport() {
    for key in [4u8, 3u8] {
        let f = Fixture::new("normal");
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 90,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_teamctl"));
        command.args([
            "--root",
            f.root.to_str().unwrap(),
            "adapter",
            "identify",
            "bridge",
        ]);
        command.env("HOME", &f.home);
        command.env("XDG_STATE_HOME", &f.state);
        command.env("PATH", &f.path);
        command.env("FIXTURE_SOURCE", "first line\nsecond line '$()'");
        command.env("TERM", "xterm-256color");
        command.env_remove("TEAMCTL_ROOT");
        command.env_remove("TMUX");
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let reading = thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
            }
        });
        f.wait("bridge", |e| {
            e.iter().any(|e| e["frame"]["type"] == "accepted")
        });
        writer.write_all(&[key]).unwrap();
        writer.flush().unwrap();
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(25));
        }
        f.wait("bridge", |e| e.iter().any(|e| e["event"] == "closed"));
        drop(writer);
        drop(pair.master);
        reading.join().unwrap();
    }
}
