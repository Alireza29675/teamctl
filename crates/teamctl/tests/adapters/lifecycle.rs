use super::fixture::{executable, wait, Fixture, Host};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn session(f: &Fixture, name: &str) -> String {
    f.applied()[name]["session"].as_str().unwrap().into()
}
fn has_session(f: &Fixture, name: &str) -> bool {
    Command::new(f.tmux.as_ref().unwrap())
        .arg("-S")
        .arg(&f.socket)
        .args(["has-session", "-t"])
        .arg(format!("={name}"))
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn named_start_stop_preserve_other_adapters_and_explicit_private_context() {
    let f = Fixture::new("normal");
    let Some(tmux) = &f.tmux else {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    };
    f.configure("normal", 2, "audit-");
    // The server predates the operator's current credentials and state base.
    assert!(Command::new(tmux)
        .arg("-S")
        .arg(&f.socket)
        .args(["new-session", "-d", "-s", "anchor", "sleep 60"])
        .env("HOME", &f.home)
        .env("PATH", &f.path)
        .env("XDG_STATE_HOME", f.temp.path().join("stale-state"))
        .env("FIXTURE_SOURCE", "stale-token")
        .output()
        .unwrap()
        .status
        .success());
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |events| {
        events.iter().any(|e| e["event"] == "started")
    });
    assert_eq!(f.applied().len(), 1);
    assert!(!f.applied().contains_key("other"));
    let started = f.events("bridge")[0].clone();
    assert_eq!(started["token_ok"], true);
    let state = std::path::PathBuf::from(started["state_dir"].as_str().unwrap());
    assert!(state.starts_with(f.state.canonicalize().unwrap()));
    let context = state.join("host-context.json");
    assert_eq!(
        fs::metadata(&context).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let carrier: Value = serde_json::from_slice(&fs::read(&context).unwrap()).unwrap();
    assert_eq!(
        carrier["env"]["FIXTURE_SOURCE"],
        "first line\nsecond line '$()'"
    );
    f.cli(&["adapter", "start", "other"]);
    f.wait("other", |e| e.iter().any(|e| e["event"] == "started"));
    let other = session(&f, "other");
    assert!(has_session(&f, &other));
    f.cli(&["adapter", "stop", "bridge"]);
    assert_eq!(f.applied().len(), 1);
    assert!(has_session(&f, &other));
    f.cli(&["adapter", "stop", "other"]);
    assert!(f.applied().is_empty());
}

#[test]
fn restart_tracks_prefix_broker_credentials_and_persisted_removal() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    let old = session(&f, "bridge");
    let old_hash = f.applied()["bridge"]["fingerprint"].clone();
    f.configure("normal", 1, "changed-");
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 2
    });
    let new = session(&f, "bridge");
    assert_ne!(old, new);
    assert!(!has_session(&f, &old));
    assert!(has_session(&f, &new));
    assert_ne!(old_hash, f.applied()["bridge"]["fingerprint"]);
    let before = f.applied()["bridge"]["fingerprint"].clone();
    let changed = f
        .command(&["adapter", "start", "bridge"])
        .env("FIXTURE_SOURCE", "rotated fixture token")
        .output()
        .unwrap();
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 3
    });
    assert_ne!(before, f.applied()["bridge"]["fingerprint"]);
    // A broker-only change replaces the host and targets the newly registered broker.
    let before = f.applied()["bridge"]["fingerprint"].clone();
    let manifest = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["broker"]["path"] = json!("custom/new-mail.db");
    fs::write(&manifest, serde_yaml::to_string(&value).unwrap()).unwrap();
    let changed_broker = team_core::chat::open(f.root.join("custom/new-mail.db")).unwrap();
    changed_broker
        .execute("INSERT INTO projects(id,name) VALUES ('p','Fixture')", [])
        .unwrap();
    changed_broker.execute("INSERT INTO agents(id,project_id,role,runtime,is_manager) VALUES ('p:manager','p','fixture','fake',1)",[]).unwrap();
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 4
    });
    assert_ne!(before, f.applied()["bridge"]["fingerprint"]);
    wait(|| {
        team_core::chat::Repository::new(&changed_broker)
            .list_conversations(10)
            .unwrap()
            .len()
            == 1
    });
    // Teardown can still use exact ownership after an invalid desired target.
    let path = f.root.join("team-compose.yaml");
    let mut yaml: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    yaml["interfaces"][0]["config"]["agent"] = json!("p:removed");
    fs::write(path, serde_yaml::to_string(&yaml).unwrap()).unwrap();
    f.cli(&["adapter", "stop", "bridge"]);
    assert!(!has_session(&f, &new));
    assert!(f.applied().is_empty());
}

#[test]
fn unchanged_start_and_stopped_recovery_keep_other_processes_owned() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    let applied = f.applied()["bridge"].clone();
    f.cli(&["adapter", "start", "bridge"]);
    assert_eq!(applied, f.applied()["bridge"]);
    assert_eq!(
        f.events("bridge")
            .iter()
            .filter(|e| e["event"] == "started")
            .count(),
        1
    );
    f.cli(&["adapter", "stop", "bridge"]);
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 2
    });
    assert_eq!(applied, f.applied()["bridge"]);
    f.cli(&["adapter", "stop", "bridge"]);
    wait(|| !has_session(&f, applied["session"].as_str().unwrap()));
}

#[test]
fn team_up_reload_down_preserve_scope_and_reconcile_without_agent_changes() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    let runtime = f.temp.path().join("fake-runtime");
    executable(&runtime, "#!/bin/sh\nexec sleep 60\n");
    fs::write(
        f.root.join("runtimes/fake.yaml"),
        format!(
            "binary: {}\nargs: []\n",
            serde_json::to_string(&runtime).unwrap()
        ),
    )
    .unwrap();
    f.configure("normal", 2, "audit-");
    let manifest = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["interfaces"][1]["config"]["agent"] = json!("p:worker");
    fs::write(&manifest, serde_yaml::to_string(&value).unwrap()).unwrap();
    f.cli(&["up", "p", "manager"]);
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    assert!(f.applied().contains_key("bridge"));
    assert!(!f.applied().contains_key("other"));
    assert!(has_session(&f, "audit-p-manager"));
    assert!(!has_session(&f, "audit-p-worker"));
    f.cli(&["up"]);
    f.wait("other", |e| e.iter().any(|e| e["event"] == "started"));
    let manager_adapter = session(&f, "bridge");
    let worker_adapter = session(&f, "other");
    assert!(Command::new(f.tmux.as_ref().unwrap())
        .arg("-S")
        .arg(&f.socket)
        .args(["kill-session", "-t"])
        .arg(format!("={manager_adapter}"))
        .output()
        .unwrap()
        .status
        .success());
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "closed"));
    f.cli(&["reload", "p", "manager"]);
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 2
    });
    assert_eq!(worker_adapter, session(&f, "other"));
    assert_eq!(
        f.events("other")
            .iter()
            .filter(|e| e["event"] == "started")
            .count(),
        1
    );
    // An interface-only removal has no agent diff and still tears down the host.
    value["interfaces"].as_array_mut().unwrap().remove(1);
    fs::write(&manifest, serde_yaml::to_string(&value).unwrap()).unwrap();
    f.cli(&["reload"]);
    assert!(!has_session(&f, &worker_adapter));
    assert!(!f.applied().contains_key("other"));
    assert!(has_session(&f, "audit-p-worker"));
    // Whole-team teardown targets the saved prefix even after manifest drift.
    value["supervisor"]["tmux_prefix"] = json!("new-");
    fs::write(&manifest, serde_yaml::to_string(&value).unwrap()).unwrap();
    f.cli(&["down"]);
    assert!(!has_session(&f, &manager_adapter));
    assert!(!has_session(&f, "audit-p-manager"));
    assert!(!has_session(&f, "audit-p-worker"));
    assert!(f.applied().is_empty());
}

#[test]
fn missing_whatsapp_assets_fail_before_agents_start() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["interfaces"][0]["type"] = json!("whatsapp");
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let output = f.command(&["up"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("matching dependencies"));
    assert!(!has_session(&f, "audit-p-manager"));
    assert!(!has_session(&f, "audit-p-worker"));
    assert!(!f.root.join("state/adapters.json").exists());
}

#[test]
fn scoped_down_stops_foreground_old_targets_after_interface_removal() {
    let f = Fixture::new("normal");
    f.configure("normal", 2, "audit-");
    fs::write(f.root.join("projects/second.yaml"),"version: 2\nproject: {id: q, name: Sibling, cwd: .}\nmanagers:\n  member: {runtime: fake}\n").unwrap();
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["projects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"file":"projects/second.yaml"}));
    value["interfaces"][1]["config"]["agent"] = json!("q:member");
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let conn = f.database();
    conn.execute("INSERT INTO projects(id,name) VALUES ('q','Sibling')", [])
        .unwrap();
    conn.execute("INSERT INTO agents(id,project_id,role,runtime,is_manager) VALUES ('q:member','q','fixture','fake',1)",[]).unwrap();
    let mut first = f.spawn();
    let mut sibling = Host::spawn(f.command(&["adapter", "run", "other"]));
    f.wait("bridge", |e| {
        e.iter().any(|e| e["frame"]["type"] == "accepted")
    });
    f.wait("other", |e| {
        e.iter().any(|e| e["frame"]["type"] == "accepted")
    });
    assert!(!f.root.join("state/adapters.json").exists());
    value["interfaces"] = json!([]);
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    f.cli(&["down", "p"]);
    assert!(first.finish().success());
    assert!(sibling.child.try_wait().unwrap().is_none());
    f.cli(&["down", "q"]);
    assert!(sibling.finish().success());
}

#[test]
fn invalid_route_scope_fails_before_worker_restart_in_live_and_dry_modes() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    let runtime = f.temp.path().join("fake-runtime");
    let launches = f.temp.path().join("runtime-launches");
    executable(
        &runtime,
        &format!(
            "#!/bin/sh\nprintf 'launch\\n' >> {}\nexec sleep 60\n",
            super::fixture::quote(launches.to_str().unwrap())
        ),
    );
    fs::write(
        f.root.join("runtimes/fake.yaml"),
        format!(
            "binary: {}\nargs: []\n",
            serde_json::to_string(&runtime).unwrap()
        ),
    )
    .unwrap();
    f.cli(&["up"]);
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    wait(|| {
        fs::read_to_string(&launches)
            .unwrap_or_default()
            .lines()
            .count()
            == 2
    });
    let applied = f.applied();
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["interfaces"][0]["config"]["agent"] = json!("p:worker");
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    for args in [
        &["reload", "p", "worker"][..],
        &["reload", "p", "worker", "--dry-run"][..],
        &["up", "p", "worker"][..],
    ] {
        let output = f.command(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("selected scope"));
        assert_eq!(fs::read_to_string(&launches).unwrap().lines().count(), 2);
        assert_eq!(f.applied(), applied);
    }
    f.cli(&["down"]);
}

#[test]
fn stale_managed_target_cannot_be_taken_over_by_a_new_foreground_target() {
    let f = Fixture::new("normal");
    if f.tmux.is_none() {
        eprintln!("tmux unavailable: lifecycle fixture skipped");
        return;
    }
    fs::write(f.root.join("projects/second.yaml"),"version: 2\nproject: {id: q, name: Sibling, cwd: .}\nmanagers:\n  member: {runtime: fake}\n").unwrap();
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["projects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"file":"projects/second.yaml"}));
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let conn = f.database();
    conn.execute("INSERT INTO projects(id,name) VALUES ('q','Sibling')", [])
        .unwrap();
    conn.execute("INSERT INTO agents(id,project_id,role,runtime,is_manager) VALUES ('q:member','q','fixture','fake',1)",[]).unwrap();
    f.cli(&["adapter", "start", "bridge"]);
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    assert!(Command::new(f.tmux.as_ref().unwrap())
        .arg("-S")
        .arg(&f.socket)
        .args(["kill-session", "-t"])
        .arg(format!("={}", session(&f, "bridge")))
        .output()
        .unwrap()
        .status
        .success());
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "closed"));
    value["interfaces"][0]["config"]["agent"] = json!("q:member");
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let takeover = f.command(&["adapter", "run", "bridge"]).output().unwrap();
    assert!(!takeover.status.success());
    assert!(String::from_utf8_lossy(&takeover.stderr).contains("prior applied identity"));
    assert_eq!(f.applied()["bridge"]["agent"], "p:manager");
    f.cli(&["adapter", "stop", "bridge"]);
    let mut current = f.spawn();
    f.wait("bridge", |e| {
        e.iter().filter(|e| e["event"] == "started").count() == 2
    });
    f.cli(&["down", "p"]);
    assert!(current.child.try_wait().unwrap().is_none());
    f.cli(&["down", "q"]);
    assert!(current.finish().success());
}

#[test]
fn stale_control_pid_without_lifetime_lock_is_not_live_and_dry_plan_creates_nothing() {
    let f = Fixture::new("normal");
    let root = f.root.canonicalize().unwrap();
    let team = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    let name = blake3::hash(b"bridge").to_hex();
    let directory = f
        .home
        .join(".config/teamctl/adapter-hosts")
        .join(&team[..24])
        .join(&name[..24]);
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let file = directory.join("control.json");
    fs::write(&file,serde_json::to_vec(&json!({"nonce":"stale","state_dir":f.state,"name":"bridge","agent":"other:old","pid":std::process::id()})).unwrap()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let owner = directory.join("owner.sqlite");
    drop(rusqlite::Connection::open(&owner).unwrap());
    let before = fs::read(&file).unwrap();
    let bytes = fs::read(&owner).unwrap();
    f.cli(&["reload", "p", "manager", "--dry-run"]);
    assert_eq!(fs::read(&file).unwrap(), before);
    assert_eq!(fs::read(&owner).unwrap(), bytes);
    fs::remove_file(&owner).unwrap();
    f.cli(&["reload", "p", "manager", "--dry-run"]);
    assert!(!owner.exists());
}
