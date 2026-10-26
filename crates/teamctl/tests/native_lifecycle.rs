//! Native lifecycle contract with private HOME and fake provider. No network/model calls.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn native_worker_replies_resumes_survives_reload_and_stops_without_tmux() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("team");
    let bin = temp.path().join("bin");
    let home = temp.path().join("home");
    let tmp = temp.path().join("tmp");
    for path in [&root, &bin, &home, &tmp] {
        fs::create_dir_all(path).unwrap();
    }
    executable(
        &bin.join("tmux"),
        "#!/bin/sh\necho 'native test must never call tmux' >&2\nexit 99\n",
    );
    executable(&bin.join("caffeinate"), "#!/bin/sh\nexit 0\n");
    executable(&bin.join("fake-codex"),"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$@\" >> \"$TEAMCTL_ROOT/args.log\"\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"fixture-session\"}' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"native fixture reply\"}}' '{\"type\":\"turn.completed\"}'\n");
    fs::create_dir(root.join("runtimes")).unwrap();
    fs::create_dir(root.join("projects")).unwrap();
    fs::write(
        root.join("runtimes/codex.yaml"),
        format!(
            "binary: {}\nsupports_mcp: true\n",
            bin.join("fake-codex").display()
        ),
    )
    .unwrap();
    fs::write(
        root.join("team-compose.yaml"),
        "version: 3.0.0\nprojects: [{file: projects/p.yaml}]\n",
    )
    .unwrap();
    fs::write(root.join("projects/p.yaml"),"version: 3\nproject: {id: nativefixture, name: Fixture, cwd: .}\nagents:\n  personal:\n    runtime: codex\n    execution: {mode: native, turn_timeout_seconds: 3}\n").unwrap();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();
    let cli = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamctl"))
            .arg("--root")
            .arg(&root)
            .args(args)
            .env("HOME", &home)
            .env("TMPDIR", &tmp)
            .env("PATH", &path)
            .env("TEAMCTL_NO_UPDATE_CHECK", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "command {args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    let process = root.join("state/processes/nativefixture-personal.json");
    // This guard is scoped to the fixture's token-owned process if an assertion fails.
    struct Stop(PathBuf, PathBuf, PathBuf, PathBuf, std::ffi::OsString);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = Command::new(&self.0)
                .arg("--root")
                .arg(&self.1)
                .arg("down")
                .env("HOME", &self.2)
                .env("TMPDIR", &self.3)
                .env("PATH", &self.4)
                .output();
        }
    }
    let _stop = Stop(
        PathBuf::from(env!("CARGO_BIN_EXE_teamctl")),
        root.clone(),
        home.clone(),
        tmp.clone(),
        path.clone(),
    );
    cli(&["up"]);
    let wait = |check: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "native fixture timed out");
            thread::sleep(Duration::from_millis(25));
        }
    };
    let session = root.join("state/runtime-sessions/nativefixture-personal.json");
    wait(&|| session.is_file());
    let before = fs::read(&process).unwrap();
    cli(&["up"]);
    assert_eq!(
        before,
        fs::read(&process).unwrap(),
        "up must preserve a running worker"
    );
    let conn = team_core::chat::open(root.join("state/mailbox.db")).unwrap();
    let inbound = team_core::chat::Repository::new(&conn)
        .ingest(team_core::chat::InboundMessage {
            adapter_id: "web".into(),
            peer_id: "operator".into(),
            sender_id: "operator".into(),
            agent_id: "nativefixture:personal".into(),
            external_id: "fixture-1".into(),
            text: "reply please".into(),
            kind: "text".into(),
            payload: None,
            title: None,
            reply_to_external_id: None,
        })
        .unwrap();
    wait(&|| {
        conn.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE direction='agent'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    });
    let history = team_core::chat::Repository::new(&conn)
        .history(&inbound.conversation_id, None, None, 20)
        .unwrap();
    assert_eq!(history[1].text, "native fixture reply");
    let inspection = cli(&["inspect", "nativefixture:personal"]);
    assert!(inspection.contains("supervisor: Running"));
    assert!(inspection.contains("execution:  native"));
    assert!(cli(&["logs", "nativefixture:personal"]).contains("native fixture reply"));
    let attach = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--root")
        .arg(&root)
        .args(["attach", "nativefixture:personal"])
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!attach.status.success());
    assert!(String::from_utf8_lossy(&attach.stderr).contains("teamctl serve"));
    assert!(fs::read_to_string(root.join("args.log"))
        .unwrap()
        .contains("resume\nfixture-session\n"));
    cli(&["reload", "--force"]);
    assert_ne!(before, fs::read(&process).unwrap());
    assert!(cli(&["status"]).contains("running"));
    // Changing the rendered desired mode must still drain the actual old
    // supervisor. A tiny fake tmux records that transition without a TUI.
    let tmux_state = root.join("tmux-state");
    let quoted = format!("'{}'", tmux_state.to_string_lossy().replace('\'', "'\\''"));
    executable(&bin.join("tmux"),&format!("#!/bin/sh\ncase \"$1\" in\n  has-session) if [ \"$3\" = '=a-nativefixture-personal' ] && [ -f {quoted} ]; then exit 0; fi; echo 'no server running' >&2; exit 1;;\n  new-session) touch {quoted};;\n  kill-session|send-keys) rm -f {quoted};;\n  *) exit 0;;\nesac\n"));
    let project = root.join("projects/p.yaml");
    let original = fs::read_to_string(&project).unwrap();
    fs::write(&project, original.replace("mode: native", "mode: tmux")).unwrap();
    cli(&["reload"]);
    assert!(!process.exists(), "native to tmux must stop old worker");
    assert!(tmux_state.exists());
    fs::write(&project, original).unwrap();
    cli(&["reload"]);
    assert!(!tmux_state.exists(), "tmux to native must stop old session");
    assert!(process.exists());
    cli(&["down"]);
    assert!(!process.exists());
    assert!(cli(&["status"]).contains("stopped"));
    assert!(
        cli(&["logs", "nativefixture:personal"]).contains("native fixture reply"),
        "stopped native logs remain inspectable"
    );
}
