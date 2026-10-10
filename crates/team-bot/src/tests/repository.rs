fn seed(conn: &Connection) {
    team_core::mailbox::ensure(conn).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name) VALUES ('p','P')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:eng_lead','p','eng_lead','claude-code',1,NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:dev1','p','dev1','claude-code',0,'eng_lead')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, project_id, role, runtime, is_manager, reports_to)
         VALUES ('p:pm','p','pm','claude-code',1,NULL)",
        [],
    )
    .unwrap();
}

#[test]
fn manager_of_returns_self_for_a_manager() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    assert_eq!(
        manager_of(&conn, "p:eng_lead").as_deref(),
        Some("p:eng_lead")
    );
    assert_eq!(manager_of(&conn, "p:pm").as_deref(), Some("p:pm"));
}

#[test]
fn manager_of_resolves_reports_to_for_a_worker() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    assert_eq!(manager_of(&conn, "p:dev1").as_deref(), Some("p:eng_lead"));
}

#[test]
fn manager_of_returns_none_for_unknown_agent() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    assert!(manager_of(&conn, "p:ghost").is_none());
}

// ── T-086-A dispatch tests ──────────────────────────────────
