/// T-036 — exercise the SQL ordering pattern used by `handle_callback`
/// (and by `cmd::approval::decide` in teamctl) directly against a
/// `Connection` so the ordering invariant has a unit-testable home.
/// Asserts: a stale tap on an `undeliverable` row does *not* flip
/// `delivered_at` (preserving the invariant
/// `undeliverable ↔ delivered_at IS NULL`), and a live tap on a
/// `pending` row flips both fields atomically.
fn decide_sql(conn: &Connection, id: i64, approved: bool) -> bool {
    let status = if approved { "approved" } else { "denied" };
    decide_approval(conn, id, status, None).unwrap()
}

fn insert_approval(conn: &Connection, status: &str, delivered_at: Option<f64>) -> i64 {
    conn.execute(
        "INSERT INTO approvals (project_id, agent_id, action, summary, status,
                                requested_at, expires_at, delivered_at)
         VALUES ('p', 'eng_lead', 'publish', 's', ?1, 0.0, strftime('%s','now')+60, ?2)",
        params![status, delivered_at],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn stale_tap_on_undeliverable_does_not_flip_delivered_at() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_approval(&conn, "undeliverable", None);

    let decided = decide_sql(&conn, id, true);
    assert!(!decided, "stale tap should report no live decision");

    let (status, delivered_at): (String, Option<f64>) = conn
        .query_row(
            "SELECT status, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "undeliverable");
    assert!(
        delivered_at.is_none(),
        "delivered_at must stay NULL on undeliverable row (invariant)"
    );
}

#[test]
fn live_tap_on_pending_flips_status_and_delivered_at() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_approval(&conn, "pending", None);

    let decided = decide_sql(&conn, id, true);
    assert!(decided, "live tap should report decision");

    let (status, delivered_at): (String, Option<f64>) = conn
        .query_row(
            "SELECT status, delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "approved");
    assert!(
        delivered_at.is_some(),
        "live decision implies delivery acknowledgement"
    );
}

/// T-039 — unscoped bot's back-compat path: when `state.manager` is
/// `None`, every approval routes to this bot regardless of which
/// agent filed it. The fallback is what makes pre-T-027 setups
/// (single team-wide bot) keep working after per-manager scoping
/// landed.
#[test]
fn unscoped_bot_routes_every_approval() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // Worker, manager, and an unknown id all route through.
    assert!(should_route(None, "p:dev1", &conn));
    assert!(should_route(None, "p:eng_lead", &conn));
    assert!(should_route(None, "p:ghost", &conn));
    // Even agents from a different (unseeded) project route through —
    // the unscoped bot is intentionally undiscriminating.
    assert!(should_route(None, "other:agent", &conn));
}

#[test]
fn scoped_bot_routes_only_its_managers_chain() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // Bot scoped to p:eng_lead. dev1 reports to eng_lead → routes.
    assert!(should_route(Some("p:eng_lead"), "p:dev1", &conn));
    // The manager themselves routes (manager_of returns self).
    assert!(should_route(Some("p:eng_lead"), "p:eng_lead", &conn));
    // pm is a sibling manager — does NOT route to eng_lead's bot.
    assert!(!should_route(Some("p:eng_lead"), "p:pm", &conn));
}

#[test]
fn scoped_bot_with_unknown_agent_falls_back_to_self_routing() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    // Unknown agent: manager_of returns None → routed = agent_id;
    // routed != scoped → does not route. This pins the fallback rule
    // (don't surface unknown rows to a scoped bot) so a future
    // change can't silently relax it.
    assert!(!should_route(Some("p:eng_lead"), "p:ghost", &conn));
}

fn insert_reply(conn: &Connection, sender: &str, text: &str) -> i64 {
    let project = sender.split_once(':').map(|(p, _)| p).unwrap_or("p");
    conn.execute(
        "INSERT INTO messages (project_id, sender, recipient, text, sent_at)
         VALUES (?1, ?2, 'user:telegram', ?3, strftime('%s','now'))",
        params![project, sender, text],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Regression: when two managers (`p:pm`, `p:eng_lead`) live in the same
/// project and each has its own scoped bot, a `reply_to_user` from one
/// manager must surface in *that* manager's bot only — not in sibling
/// bots. Pre-fix the project-id SQL filter was the only filter so all
/// in-project bots fanned out the same reply.
#[test]
fn reply_routes_only_to_its_senders_bot() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let pm_msg = insert_reply(&conn, "p:pm", "from pm");
    let eng_msg = insert_reply(&conn, "p:eng_lead", "from eng");

    // Pull the project-scoped pre-filter rows the way outbound_loop does.
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.sender, m.text FROM messages m
             WHERE m.id > 0
               AND m.recipient = 'user:telegram'
               AND m.acked_at IS NULL
               AND m.project_id = 'p'
             ORDER BY m.id",
        )
        .unwrap();
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(rows.len(), 2, "both replies share the project pre-filter");

    // pm bot keeps only the pm reply.
    let pm_routed: Vec<i64> = rows
        .iter()
        .filter(|(_, sender, _)| should_route(Some("p:pm"), sender, &conn))
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(pm_routed, vec![pm_msg]);

    // eng_lead bot keeps only the eng_lead reply.
    let eng_routed: Vec<i64> = rows
        .iter()
        .filter(|(_, sender, _)| should_route(Some("p:eng_lead"), sender, &conn))
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(eng_routed, vec![eng_msg]);

    // Unscoped bot back-compat: forwards both.
    let unscoped: Vec<i64> = rows
        .iter()
        .filter(|(_, sender, _)| should_route(None, sender, &conn))
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(unscoped, vec![pm_msg, eng_msg]);
}

#[test]
fn live_tap_keeps_existing_delivered_at_unchanged() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);
    let id = insert_approval(&conn, "pending", Some(1234.5));

    let decided = decide_sql(&conn, id, false);
    assert!(decided);

    let delivered_at: f64 = conn
        .query_row(
            "SELECT delivered_at FROM approvals WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        (delivered_at - 1234.5).abs() < 1e-6,
        "previously-set delivered_at must not be overwritten ({delivered_at})"
    );
}

// ── T-086-G slash-passthrough ─────────────────────────────────
