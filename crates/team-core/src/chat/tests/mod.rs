use super::*;

fn connection() -> rusqlite::Connection {
    let conn = open(":memory:").unwrap();
    conn.execute_batch("INSERT INTO projects(id,name) VALUES ('p','P'); INSERT INTO agents(id,project_id,role,runtime) VALUES ('p:a','p','worker','fake'),('p:b','p','worker','fake');").unwrap();
    conn
}

fn inbound(peer: &str, external: &str) -> InboundMessage {
    InboundMessage {
        adapter_id: "telegram:p:a".into(),
        peer_id: peer.into(),
        sender_id: "human-1".into(),
        agent_id: "p:a".into(),
        external_id: external.into(),
        text: "hello".into(),
        kind: "text".into(),
        payload: None,
        title: None,
        reply_to_external_id: None,
    }
}

fn count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn existing(conn: &rusqlite::Connection, inbound: &InboundMessage) -> i64 {
    conn.execute("INSERT INTO messages(project_id, sender, recipient, text, kind, structured_payload, telegram_msg_id, sent_at) VALUES ('p', 'user:telegram', ?1, ?2, ?3, ?4, 42, 1)", rusqlite::params![inbound.agent_id, inbound.text, inbound.kind, inbound.payload.as_ref().map(ToString::to_string)]).unwrap();
    conn.last_insert_rowid()
}

mod contract;
mod conversations;
mod delivery;
mod inbound;
