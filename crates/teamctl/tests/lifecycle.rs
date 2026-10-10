//! Real tmux + fake runtime contract. Every process uses a private socket,
//! HOME, TMPDIR and compose root; no paid AI sessions or operator teams.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    root: PathBuf,
    tmux: PathBuf,
    socket: PathBuf,
    path: std::ffi::OsString,
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Fixture {
    fn new() -> Option<Self> {
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let tmux = std::env::split_paths(&original_path)
            .map(|p| p.join("tmux"))
            .find(|p| p.is_file())?;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("team with 'quotes'");
        let bin = tmp.path().join("bin");
        let socket = tmp.path().join("tmux.sock");
        for dir in [
            &root,
            &bin,
            &tmp.path().join("home"),
            &tmp.path().join("tmp"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        executable(
            &bin.join("tmux"),
            &format!(
                "#!/bin/sh\nexec {} -S {} \"$@\"\n",
                quote(tmux.to_str().unwrap()),
                quote(socket.to_str().unwrap())
            ),
        );
        // Avoid host-level power policy in the fixture.
        executable(&bin.join("caffeinate"), "#!/bin/sh\nexit 0\n");
        executable(&bin.join("fake-runtime"),
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$AGENT_ID\" \"$MODEL\" \"$LABEL\"\nprintf '%s\\n' \"$@\"\nexec sleep 60\n");
        fs::create_dir(root.join("projects")).unwrap();
        fs::create_dir(root.join("runtimes")).unwrap();
        fs::write(root.join("team-compose.yaml"),
            "version: 2.0.0\nbroker: {type: sqlite, path: custom/mail.db}\nsupervisor: {type: tmux, tmux_prefix: audit-, drain_timeout_secs: 0}\nprojects: [{file: projects/fixture.yaml}]\n").unwrap();
        fs::write(root.join("runtimes/fake.yaml"),
            format!("binary: {}\nargs: ['argument with spaces', \"literal 'quote'\"]\ndefault_model: fixture-model\nenv: {{LABEL: fixture-label}}\n",
            serde_json::to_string(bin.join("fake-runtime").to_str().unwrap()).unwrap())).unwrap();
        let mut paths = vec![
            bin,
            PathBuf::from(env!("CARGO_BIN_EXE_teamctl"))
                .parent()
                .unwrap()
                .to_path_buf(),
        ];
        paths.extend(std::env::split_paths(&original_path));
        let path = std::env::join_paths(paths).unwrap();
        let fixture = Self {
            tmp,
            root,
            tmux,
            socket,
            path,
        };
        fixture.project(None, None);
        Some(fixture)
    }

    fn project(&self, a_model: Option<&str>, b_model: Option<&str>) {
        let model = |s: Option<&str>| s.map(|s| format!("\n    model: {s}")).unwrap_or_default();
        fs::write(self.root.join("projects/fixture.yaml"), format!(
            "version: 2\nproject: {{id: fixture, name: Fixture, cwd: .}}\nmanagers:\n  a:\n    runtime: fake{}\nworkers:\n  b:\n    runtime: fake{}\n    reports_to: a\n", model(a_model), model(b_model))).unwrap();
    }

    fn command_at(&self, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_teamctl"))
            .arg("--root")
            .arg(root)
            .args(args)
            .env("PATH", &self.path)
            .env("HOME", self.tmp.path().join("home"))
            .env("TMPDIR", self.tmp.path().join("tmp"))
            .env("TEAMCTL_TEAM_BOT", self.tmp.path().join("bin/team-bot"))
            .env("FIXTURE_BOT_TOKEN", "fixture-token")
            .env("FIXTURE_BOT_CHATS", "1")
            .env_remove("TEAMCTL_ROOT")
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn cli(&self, args: &[&str]) -> Output {
        let out = self.command_at(&self.root, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn tmux(&self, args: &[&str]) -> Output {
        Command::new(&self.tmux)
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap()
    }

    fn wait_capture(&self, session: &str, expected: &str) {
        let start = Instant::now();
        loop {
            let out = self.tmux(&["capture-pane", "-p", "-t", session]);
            if String::from_utf8_lossy(&out.stdout).contains(expected) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "did not see {expected} in {session}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

#[test]
fn scoped_start_snapshot_reconciliation_and_persisted_teardown() {
    let Some(f) = Fixture::new() else {
        eprintln!("tmux unavailable: real lifecycle contract skipped (CI installs tmux)");
        return;
    };
    // First-ever scoped start must create infrastructure, honor descriptor
    // defaults/env/argv, and register the selected project.
    f.cli(&["up", "fixture"]);
    f.wait_capture("audit-fixture-a", "fixture:a|fixture-model|fixture-label");
    f.wait_capture("audit-fixture-a", "argument with spaces");
    f.wait_capture("audit-fixture-b", "literal 'quote'");
    assert!(f.root.join("custom/mail.db").exists());
    assert!(f.root.join("state/applied.json").exists());

    f.project(Some("model-a"), Some("model-b"));
    f.cli(&["up", "fixture"]); // running sessions retain prior applied state
    let dry = f.cli(&["reload", "--dry-run"]);
    let dry = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry.contains("fixture:a") && dry.contains("fixture:b"),
        "{dry}"
    );

    f.cli(&["reload", "fixture", "a", "--force"]);
    f.wait_capture("audit-fixture-a", "fixture:a|model-a|fixture-label");
    let dry = f.cli(&["reload", "--dry-run"]);
    let dry = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry.contains("fixture:b"),
        "unselected desired change disappeared: {dry}"
    );
    f.cli(&["reload", "--force"]);
    f.wait_capture("audit-fixture-b", "fixture:b|model-b|fixture-label");

    assert!(f
        .tmux(&["kill-session", "-t", "=audit-fixture-b"])
        .status
        .success());
    f.cli(&["reload"]); // unchanged config must recover the stopped session
    f.wait_capture("audit-fixture-b", "fixture:b|model-b|fixture-label");

    let manifest = f.root.join("team-compose.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("audit-", "new-"),
    )
    .unwrap();
    f.cli(&["down"]); // teardown reads old session identities from applied state
    assert!(!f
        .tmux(&["has-session", "-t", "=audit-fixture-a"])
        .status
        .success());
    assert!(!f
        .tmux(&["has-session", "-t", "=audit-fixture-b"])
        .status
        .success());
}

#[test]
fn idle_runtime_rate_limit_without_newline_is_recorded_and_terminated() {
    let Some(f) = Fixture::new() else {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    };
    let manifest = f.root.join("team-compose.yaml");
    let mut yaml = fs::read_to_string(&manifest).unwrap();
    yaml.push_str("rate_limits: {fallback_wait_seconds: 0}\n");
    fs::write(manifest, yaml).unwrap();
    let descriptor = f.root.join("runtimes/fake.yaml");
    let mut yaml = fs::read_to_string(&descriptor).unwrap();
    yaml.push_str("rate_limit_patterns:\n  - match: 'FIXTURE_LIMIT'\n");
    fs::write(descriptor, yaml).unwrap();
    let db = f.root.join("custom/mail.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    let script = f.tmp.path().join("rate-runtime");
    let pidfile = f.tmp.path().join("rate.pid");
    executable(
        &script,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nprintf FIXTURE_LIMIT\nexec sleep 60\n",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--root")
        .arg(&f.root)
        .args(["rl-watch", "fixture:a", "--"])
        .arg(&script)
        .arg(&pidfile)
        .env("PATH", &f.path)
        .env("HOME", f.tmp.path().join("home"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rate-limit watcher waited for the idle runtime to exit");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rate_limits WHERE agent_id='fixture:a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let pid: i32 = fs::read_to_string(&pidfile).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "owned runtime must be terminated and reaped"
    );
}

#[test]
fn different_roots_cannot_adopt_or_stop_a_colliding_concrete_session() {
    let Some(f) = Fixture::new() else {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    };
    fs::write(
        f.root.join("projects/fixture.yaml"),
        "version: 2\nproject: {id: a-b, name: First, cwd: .}\nmanagers:\n  c: {runtime: fake}\n",
    )
    .unwrap();
    f.cli(&["up"]);
    f.wait_capture("audit-a-b-c", "a-b:c|fixture-model|fixture-label");
    let other = f.tmp.path().join("other-root");
    fs::create_dir_all(other.join("projects")).unwrap();
    fs::create_dir_all(other.join("runtimes")).unwrap();
    fs::copy(
        f.root.join("team-compose.yaml"),
        other.join("team-compose.yaml"),
    )
    .unwrap();
    fs::copy(
        f.root.join("runtimes/fake.yaml"),
        other.join("runtimes/fake.yaml"),
    )
    .unwrap();
    fs::write(
        other.join("projects/fixture.yaml"),
        "version: 2\nproject: {id: a, name: Second, cwd: .}\nmanagers:\n  b-c: {runtime: fake}\n",
    )
    .unwrap();
    let start = f.command_at(&other, &["up"]);
    assert!(
        !start.status.success(),
        "second root adopted someone else's session"
    );
    let stop = f.command_at(&other, &["down"]);
    assert!(
        !stop.status.success(),
        "second root must reject teardown of someone else's session"
    );
    assert!(f
        .tmux(&["has-session", "-t", "=audit-a-b-c"])
        .status
        .success());
    f.cli(&["down"]);
}

#[test]
fn removed_legacy_bot_without_identity_state_is_reaped_by_reload_and_down() {
    for operation in ["reload", "down"] {
        let Some(f) = Fixture::new() else {
            eprintln!("tmux unavailable: legacy bot fixture skipped");
            return;
        };
        executable(
            &f.tmp.path().join("bin/team-bot"),
            "#!/bin/sh\nprintf 'fixture legacy bot\\n'\nexec sleep 60\n",
        );
        let project = f.root.join("projects/fixture.yaml");
        let original = fs::read_to_string(&project).unwrap();
        fs::write(
            &project,
            original.replace(
                "    runtime: fake\nworkers:",
                "    runtime: fake\n    interfaces:\n      telegram:\n        bot_token_env: FIXTURE_BOT_TOKEN\n        chat_ids_env: FIXTURE_BOT_CHATS\nworkers:",
            ),
        )
        .unwrap();
        f.cli(&["up"]);
        f.wait_capture("audit-bot-fixture-a", "fixture legacy bot");
        // Emulate a pre-upgrade running bot: its launch command and the
        // prior applied manager are the only ownership evidence.
        fs::remove_file(f.root.join("state/bots.json")).unwrap();
        fs::remove_file(f.root.join("state/bot-inputs.json")).unwrap();
        fs::write(&project, original).unwrap();
        f.cli(&[operation]);
        assert!(
            !f.tmux(&["has-session", "-t", "=audit-bot-fixture-a"])
                .status
                .success(),
            "{operation} left the removed legacy bot alive"
        );
        if operation == "reload" {
            assert!(f
                .tmux(&["has-session", "-t", "=audit-fixture-a"])
                .status
                .success());
            f.cli(&["down"]);
        }
    }
}
