use rusqlite::params;
use serde_json::json;
use team_core::{
    chat::{self, DeliveryStatus, Repository},
    compose::Compose,
    interfaces,
};

fn fixture() -> (tempfile::TempDir, Compose, rusqlite::Connection) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("projects")).unwrap();
    std::fs::write(
        root.path().join("team-compose.yaml"),
        r#"version: "3.0.0"
projects:
  - file: projects/p.yaml
interfaces:
  - id: personal
    provider: telegram
    name: Personal bot
    description: Private errands
    config: {agent: 'p:assistant',bot_token_env: TOKEN,authorized_chat_ids_env: CHATS}
  - id: work
    provider: telegram
    name: Work bot
    description: Work only
    config: {agent: 'p:assistant',bot_token_env: WORK_TOKEN,authorized_chat_ids_env: WORK_CHATS}
"#,
    )
    .unwrap();
    std::fs::write(root.path().join("projects/p.yaml"),"version: 2\nproject: {id: p,name: Personal,cwd: .}\nagents:\n  assistant: {runtime: codex}\n  helper: {runtime: claude-code, reports_to: assistant}\n").unwrap();
    let compose = Compose::load_read_only(root.path()).unwrap();
    let conn = chat::open(root.path().join("state/mailbox.db")).unwrap();
    conn.execute("INSERT INTO projects VALUES('p','Personal')", [])
        .unwrap();
    for name in ["assistant", "helper"] {
        conn.execute(
            "INSERT INTO agents(id,project_id,role,runtime) VALUES(?1,'p',?2,'codex')",
            params![format!("p:{name}"), name],
        )
        .unwrap();
    }
    interfaces::sync(&conn, &compose).unwrap();
    (root, compose, conn)
}

#[test]
fn two_bots_have_independent_identity_and_web_is_automatic() {
    let (_root, compose, conn) = fixture();
    assert!(team_core::validate::validate(&compose).is_empty());
    let all = interfaces::list_for_agent(&conn, "p:assistant").unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter().find(|i| i.id == "work").unwrap().description,
        "Work only"
    );
    assert_eq!(
        interfaces::list_for_agent(&conn, "p:helper").unwrap().len(),
        1
    );
    let yaml = serde_yaml::to_string(&compose.projects[0]).unwrap();
    assert!(yaml.contains("agents:"));
    assert!(!yaml.contains("managers:"));
}

#[test]
fn rich_web_round_trip_snapshots_provenance_and_survives_inbox_cleanup() {
    let (_root, _compose, conn) = fixture();
    let repo = Repository::new(&conn);
    let c = repo
        .open_interface("p:helper", "web", "operator", "Helper")
        .unwrap();
    let inbound = repo
        .send_rich(
            &c.id,
            "Look",
            "image",
            Some(json!({"attachments":[{"id":"asset","url":"/assets/asset"}]})),
            "one",
        )
        .unwrap();
    assert_eq!(
        inbound,
        repo.send_rich(
            &c.id,
            "Look",
            "image",
            Some(json!({"attachments":[{"id":"asset","url":"/assets/asset"}]})),
            "one"
        )
        .unwrap()
    );
    let info: String = conn
        .query_row(
            "SELECT interface_json FROM messages WHERE id=?1",
            [inbound.mailbox_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&info).unwrap()["provider"],
        "web"
    );
    let reply = repo
        .reply(
            "p:helper",
            None,
            inbound.mailbox_id,
            "",
            "html",
            Some(json!({"html":"<button>Hi</button>"})),
            Some("reply"),
        )
        .unwrap();
    assert_eq!(reply.status, DeliveryStatus::Sent);
    conn.execute("DELETE FROM messages", []).unwrap();
    assert_eq!(repo.history(&c.id, None, None, 20).unwrap().len(), 2);
}

#[test]
fn revoked_interface_and_cross_agent_routes_are_rejected() {
    let (_root, mut compose, conn) = fixture();
    let repo = Repository::new(&conn);
    assert!(repo
        .open_interface("p:helper", "personal", "7", "Private")
        .is_err());
    let c = repo
        .open_interface("p:assistant", "personal", "7", "Private")
        .unwrap();
    assert!(repo
        .reply("p:helper", Some(&c.id), None, "no", "text", None, None)
        .is_err());
    compose.global.interfaces.clear();
    interfaces::sync(&conn, &compose).unwrap();
    assert!(repo
        .reply(
            "p:assistant",
            Some(&c.id),
            None,
            "stale",
            "text",
            None,
            None
        )
        .is_err());
}

#[test]
fn hierarchy_rejects_self_and_cycles() {
    let (_root, mut compose, _conn) = fixture();
    compose.projects[0]
        .managers
        .get_mut("assistant")
        .unwrap()
        .reports_to = Some("helper".into());
    assert!(team_core::validate::validate(&compose)
        .iter()
        .any(|e| matches!(e, team_core::validate::ValidationError::Hierarchy(_))));
    compose.projects[0]
        .managers
        .get_mut("assistant")
        .unwrap()
        .reports_to = None;
    compose.projects[0]
        .managers
        .get_mut("helper")
        .unwrap()
        .reports_to = Some("helper".into());
    assert!(team_core::validate::validate(&compose)
        .iter()
        .any(|e| matches!(e, team_core::validate::ValidationError::Hierarchy(_))));
}

#[test]
fn attachment_paths_are_visible_to_agent_without_polluting_chat_text() {
    let (_root, _compose, conn) = fixture();
    let repo = Repository::new(&conn);
    let c = repo
        .open_interface("p:helper", "web", "operator", "Helper")
        .unwrap();
    let m = repo
        .send_rich(
            &c.id,
            "Photo",
            "image",
            Some(json!({"attachments":[{"id":"asset","path":"/tmp/photo.png"}]})),
            "photo",
        )
        .unwrap();
    assert_eq!(m.text, "Photo");
    let inbox: String = conn
        .query_row(
            "SELECT text FROM messages WHERE id=?1",
            [m.mailbox_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(inbox, "Photo\n📎 attachment: /tmp/photo.png");
}

#[test]
fn queued_deliveries_fail_when_interface_is_revoked_or_reassigned() {
    for reassign in [false, true] {
        let (_root, _compose, conn) = fixture();
        let repo = Repository::new(&conn);
        let c = repo
            .open_interface("p:assistant", "personal", "7", "Private")
            .unwrap();
        let message = repo
            .reply(
                "p:assistant",
                Some(&c.id),
                None,
                "private reply",
                "text",
                None,
                None,
            )
            .unwrap();
        conn.execute(
            "DELETE FROM agent_interfaces WHERE interface_id='personal'",
            [],
        )
        .unwrap();
        if reassign {
            conn.execute(
                "INSERT INTO agent_interfaces VALUES('p:helper','personal')",
                [],
            )
            .unwrap();
        } else {
            conn.execute(
                "UPDATE interface_instances SET active=0 WHERE id='personal'",
                [],
            )
            .unwrap();
        }
        assert!(repo
            .resolve_route("p:assistant", Some(&c.id), None)
            .is_err());
        assert!(repo
            .claim_deliveries("personal", "bot", f64::MAX / 2.0, 30.0, 20)
            .unwrap()
            .is_empty());
        let failed = repo.message(message.id).unwrap();
        assert_eq!(failed.status, DeliveryStatus::Failed);
        assert!(failed.error.unwrap().contains("revoked"));
    }
}

#[test]
fn immutable_interface_metadata_survives_rename_removal_and_inbox_gc() {
    let (_root, _compose, conn) = fixture();
    let repo = Repository::new(&conn);
    let c = repo
        .open_interface("p:helper", "web", "operator", "Helper")
        .unwrap();
    let inbound = repo
        .send_rich(&c.id, "hello", "text", None, "source")
        .unwrap();
    let outbound = repo
        .reply(
            "p:helper",
            Some(&c.id),
            inbound.mailbox_id,
            "reply",
            "text",
            None,
            None,
        )
        .unwrap();
    let original = inbound.interface.unwrap();
    assert_eq!(outbound.interface.as_ref(), Some(&original));
    conn.execute("UPDATE interface_instances SET name='Renamed',description='Changed',active=0 WHERE id='web'",[]).unwrap();
    conn.execute("DELETE FROM messages", []).unwrap();
    assert!(repo
        .history(&c.id, None, None, 20)
        .unwrap()
        .iter()
        .all(|m| m.interface.as_ref() == Some(&original)));
}
