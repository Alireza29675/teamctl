use super::fixture::{wait, Fixture};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use team_core::chat::{self, Repository};

#[test]
fn real_protocol_scopes_ingress_deduplicates_and_confirms_only_current_delivery() {
    let f = Fixture::new("normal");
    let mut host = f.spawn();
    f.wait("bridge", |events| {
        events
            .iter()
            .filter(|event| event["frame"]["type"] == "accepted")
            .count()
            == 2
    });
    let conn = f.database();
    let repo = Repository::new(&conn);
    let conversations = repo.list_conversations(20).unwrap();
    assert_eq!(conversations.len(), 1);
    assert!(String::from_utf8_lossy(&f.cli(&["adapter", "list"]).stdout)
        .contains("bridge  process  p:manager  Running"));
    assert_eq!(
        repo.history(&conversations[0].id, None, None, 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repo.capabilities("bridge").unwrap().unwrap(),
        vec!["text", "quoted_reply"]
    );
    let reply = repo
        .reply(
            "p:manager",
            Some(&conversations[0].id),
            None,
            "hello back",
            "text",
            None,
            Some("reply:1"),
        )
        .unwrap();
    wait(|| {
        repo.history(&conversations[0].id, None, None, 20)
            .unwrap()
            .iter()
            .any(|message| message.id == reply.id && message.status == chat::DeliveryStatus::Sent)
    });
    let events = f.events("bridge");
    assert!(events
        .iter()
        .any(|event| event["frame"]["code"] == "forbidden_sender"));
    assert!(events[0]["token_ok"].as_bool().unwrap());
    assert_eq!(events[0]["unrelated_leaked"], false);
    let state = PathBuf::from(events[0]["state_dir"].as_str().unwrap());
    assert!(!state.starts_with(f.root.canonicalize().unwrap()));
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    f.stop();
    assert!(host.finish().success());
    assert!(!host.text().contains("DO_NOT_DISPLAY"));
    f.wait("bridge", |events| {
        events.iter().any(|event| event["event"] == "closed")
    });
}

#[test]
fn retry_preserves_stable_request_id_and_sanitizes_provider_errors() {
    let f = Fixture::new("retry");
    let mut host = f.spawn();
    f.wait("bridge", |events| {
        events
            .iter()
            .any(|event| event["frame"]["type"] == "accepted")
    });
    let conn = f.database();
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().remove(0);
    let reply = repo
        .reply(
            "p:manager",
            Some(&conversation.id),
            None,
            "retry reply",
            "text",
            None,
            Some("r"),
        )
        .unwrap();
    wait(|| {
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .iter()
            .any(|m| m.id == reply.id && m.status == chat::DeliveryStatus::Sent)
    });
    let requests: Vec<_> = f
        .events("bridge")
        .into_iter()
        .filter(|e| e["frame"]["type"] == "send")
        .map(|e| e["frame"]["request_id"].clone())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    f.stop();
    assert!(host.finish().success());
    assert!(!host.text().contains("PRIVATE_PROVIDER_ERROR"));
}

#[test]
fn reconnect_after_original_deadline_uses_connected_without_a_second_ready() {
    let f = Fixture::new("reconnect");
    let mut host = f.spawn();
    f.wait("bridge", |events| {
        events.iter().any(|e| e["event"] == "reconnected")
    });
    let conn = f.database();
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().remove(0);
    let reply = repo
        .reply(
            "p:manager",
            Some(&conversation.id),
            None,
            "after reconnect",
            "text",
            None,
            None,
        )
        .unwrap();
    wait(|| {
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .iter()
            .any(|m| m.id == reply.id && m.status == chat::DeliveryStatus::Sent)
    });
    f.stop();
    assert!(host.finish().success());
}

#[test]
fn duplicate_host_is_rejected_immediately_even_if_state_base_changes() {
    let f = Fixture::new("normal");
    let mut first = f.spawn();
    f.wait("bridge", |e| e.iter().any(|e| e["event"] == "started"));
    let start = Instant::now();
    let second = f
        .command(&["adapter", "run", "bridge"])
        .env("XDG_STATE_HOME", f.temp.path().join("other-state"))
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(String::from_utf8_lossy(&second.stderr).contains("profile is in use"));
    f.stop();
    assert!(first.finish().success());
}

#[test]
fn startup_invalid_frames_and_background_pairing_fail_closed_without_secret_output() {
    for mode in [
        "silent",
        "before_ready",
        "qr",
        "malformed",
        "version",
        "oversized",
    ] {
        let f = Fixture::new(mode);
        let start = Instant::now();
        let mut host = f.spawn();
        assert!(!host.finish().success(), "mode {mode}");
        assert!(start.elapsed() < Duration::from_secs(7));
        assert!(!host.text().contains("FAKE_QR_SECRET_DO_NOT_LOG"));
        let conn = f.database();
        assert_eq!(
            Repository::new(&conn).list_conversations(10).unwrap().len(),
            0
        );
    }
}

#[test]
fn reserved_instances_private_state_and_tty_admin_gates_are_enforced() {
    let f = Fixture::new("normal");
    for name in ["local", "telegram:p:manager"] {
        let path = f.root.join("team-compose.yaml");
        let mut yaml: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
        yaml["interfaces"][0]["name"] = json!(name);
        fs::write(&path, serde_yaml::to_string(&yaml).unwrap()).unwrap();
        let output = f.command(&["adapter", "list"]).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("reserved"));
    }
    f.configure("normal", 1, "audit-");
    for command in ["pair", "identify"] {
        let out = f.command(&["adapter", command, "bridge"]).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));
    }
    let out = f
        .command(&["adapter", "run", "bridge"])
        .env("XDG_STATE_HOME", f.root.join("unsafe-state"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("outside"));
    assert!(!f.root.join("unsafe-state").exists());
}

#[test]
fn host_and_actual_node_sdk_echo_replay_and_acknowledgements_round_trip() {
    let f = Fixture::new("normal");
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("node"))
        .find(|path| path.is_file());
    let Some(node) = node else {
        eprintln!("Node unavailable: SDK executable fixture skipped");
        return;
    };
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/examples/echo.js")
        .canonicalize()
        .unwrap();
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["interfaces"][0]["config"]["command"] = json!(node);
    value["interfaces"][0]["config"]["args"] = json!([script]);
    value["interfaces"][0]["config"]["allow_senders"] = json!(["echo-user"]);
    value["interfaces"][0]["config"]["options"] =
        json!({"peer_id":"peer:one","greeting":"SDK greeting"});
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let conn = f.database();
    let repo = Repository::new(&conn);
    let mut host = f.spawn();
    wait(|| repo.list_conversations(10).unwrap().len() == 1);
    let conversation = repo.list_conversations(10).unwrap().remove(0);
    let reply = repo
        .reply(
            "p:manager",
            Some(&conversation.id),
            None,
            "SDK reply",
            "text",
            None,
            Some("sdk:reply"),
        )
        .unwrap();
    wait(|| {
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .iter()
            .any(|m| m.id == reply.id && m.status == chat::DeliveryStatus::Sent)
    });
    wait(|| {
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .len()
            == 3
    });
    f.stop();
    assert!(host.finish().success());
    let sdk_state = walk_sdk_state(&f.state);
    let contents: Value = serde_json::from_slice(&fs::read(sdk_state).unwrap()).unwrap();
    assert!(contents["inbound"].as_array().unwrap().is_empty());
    assert_eq!(contents["receipts"].as_array().unwrap().len(), 1);
    let mut again = f.spawn();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .len(),
        3
    );
    f.stop();
    assert!(again.finish().success());
}

fn walk_sdk_state(directory: &std::path::Path) -> std::path::PathBuf {
    find_sdk_state(directory).expect("SDK state missing")
}
fn find_sdk_state(directory: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_name() == "adapter-state.json" {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(path) = find_sdk_state(&path) {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn expired_or_malformed_typing_is_not_forwarded_and_live_status_is_sent() {
    let f = Fixture::new("normal");
    let path = f.root.join("team-compose.yaml");
    let mut value: Value = serde_yaml::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["interfaces"][0]["config"]["options"]["capabilities"] =
        json!(["text", "quoted_reply", "typing"]);
    fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
    let mut host = f.spawn();
    f.wait("bridge", |e| {
        e.iter().any(|e| e["frame"]["type"] == "accepted")
    });
    let conn = f.database();
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().remove(0);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let mut rejected = Vec::new();
    for payload in [
        None,
        Some(json!({"expires_at":"bad"})),
        Some(json!({"expires_at":clock-1.0})),
        Some(json!({"expires_at":clock+60.0})),
    ] {
        rejected.push(
            repo.reply(
                "p:manager",
                Some(&conversation.id),
                None,
                "",
                "typing",
                payload,
                None,
            )
            .unwrap()
            .id,
        );
    }
    wait(|| {
        repo.history(&conversation.id, None, None, 20)
            .unwrap()
            .iter()
            .filter(|m| rejected.contains(&m.id) && m.status == chat::DeliveryStatus::Failed)
            .count()
            == 4
    });
    assert!(!f
        .events("bridge")
        .iter()
        .any(|e| e["frame"]["type"] == "send"));
    let live=repo.reply("p:manager",Some(&conversation.id),None,"","typing",Some(json!({"expires_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()+5.0})),None).unwrap();
    wait(|| {
        repo.history(&conversation.id, None, None, 20)
            .unwrap()
            .iter()
            .any(|m| m.id == live.id && m.status == chat::DeliveryStatus::Sent)
    });
    f.stop();
    assert!(host.finish().success());
}

#[test]
fn stopping_an_unconfirmed_delivery_releases_its_lease_for_restart() {
    let f = Fixture::new("hang_send");
    let mut first = f.spawn();
    f.wait("bridge", |e| {
        e.iter().any(|e| e["frame"]["type"] == "accepted")
    });
    let conn = f.database();
    let repo = Repository::new(&conn);
    let conversation = repo.list_conversations(10).unwrap().remove(0);
    let reply = repo
        .reply(
            "p:manager",
            Some(&conversation.id),
            None,
            "pending reply",
            "text",
            None,
            None,
        )
        .unwrap();
    f.wait("bridge", |e| e.iter().any(|e| e["frame"]["type"] == "send"));
    f.stop();
    assert!(first.finish().success());
    let lease: Option<String> = conn
        .query_row(
            "SELECT lease_owner FROM chat_deliveries WHERE message_id=?1",
            [reply.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(lease.is_none());
    f.configure("normal", 1, "audit-");
    let mut again = f.spawn();
    wait(|| {
        repo.history(&conversation.id, None, None, 10)
            .unwrap()
            .iter()
            .any(|m| m.id == reply.id && m.status == chat::DeliveryStatus::Sent)
    });
    f.stop();
    assert!(again.finish().success());
}

#[test]
fn owned_group_helpers_cannot_survive_leader_exit_or_shutdown() {
    for mode in ["fork_exit", "fork_shutdown", "fork_term"] {
        let f = Fixture::new(mode);
        let mut host = f.spawn();
        f.wait("bridge", |e| e.iter().any(|e| e["event"] == "helper"));
        let pid = f
            .events("bridge")
            .iter()
            .find(|e| e["event"] == "helper")
            .unwrap()["pid"]
            .as_i64()
            .unwrap() as i32;
        if mode != "fork_exit" {
            f.stop();
            assert!(host.finish().success());
        } else {
            assert!(!host.finish().success());
        }
        wait(|| unsafe { libc::kill(pid, 0) == -1 });
    }
}
