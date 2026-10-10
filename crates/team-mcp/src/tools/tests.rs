use super::*;
use rusqlite::params;
use tempfile::NamedTempFile;

fn ctx_with_manager() -> (Ctx, NamedTempFile) {
    let f = NamedTempFile::new().unwrap();
    let store = Store::open(f.path()).unwrap();
    store
        .upsert_agent("p:mgr", "p", "P", "manager", "claude-code", true)
        .unwrap();
    (Ctx::new("p:mgr".to_string(), store, "t-".to_string()), f)
}

fn fetch_message(store: &Store, id: i64) -> (String, Option<String>, Option<String>) {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT text, kind, structured_payload FROM messages WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

include!("tests/reply.rs");
include!("tests/controls.rs");
include!("tests/tmux.rs");
include!("tests/threading.rs");
include!("tests/attachments.rs");
include!("tests/approvals.rs");
