use super::*;

#[test]
fn delivery_owner_expiration_and_status_changes_are_observable() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = r.ingest(inbound("peer", "42")).unwrap();
    let reply = r
        .reply("p:a", None, input.mailbox_id, "reply", "text", None, None)
        .unwrap();
    let before = r.get_conversation(&reply.conversation_id).unwrap();
    let now = super::repository::now();
    let claimed = r
        .claim_deliveries("telegram:p:a", "worker-a", now, 30.0, 10)
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].reply_to_external_id.as_deref(), Some("42"));
    assert_eq!(claimed[0].peer_id, "peer");
    assert!(r
        .claim_deliveries("telegram:p:a", "worker-b", now, 30.0, 10)
        .unwrap()
        .is_empty());
    assert!(matches!(
        r.mark_sent(claimed[0].id, "worker-b", Some("sent")),
        Err(Error::LeaseLost)
    ));
    conn.execute("UPDATE chat_deliveries SET lease_until = ?1", [now - 1.0])
        .unwrap();
    assert!(matches!(
        r.mark_sent(claimed[0].id, "worker-a", Some("sent")),
        Err(Error::LeaseLost)
    ));
    let reclaimed = r
        .claim_deliveries("telegram:p:a", "worker-b", now, 30.0, 10)
        .unwrap();
    assert_eq!(reclaimed[0].id, claimed[0].id);
    assert_eq!(reclaimed[0].attempts, 2);
    assert!(matches!(
        r.mark_failed(claimed[0].id, "worker-a", "late", false),
        Err(Error::LeaseLost)
    ));
    r.mark_sent(reclaimed[0].id, "worker-b", Some("provider-99"))
        .unwrap();
    let after = r.get_conversation(&reply.conversation_id).unwrap();
    assert!(after.revision > before.revision);
    let history = r.history(&reply.conversation_id, None, None, 10).unwrap();
    assert_eq!(history.last().unwrap().id, reply.id);
    assert_eq!(history.last().unwrap().status, DeliveryStatus::Sent);
    assert_eq!(
        history.last().unwrap().external_id.as_deref(),
        Some("provider-99")
    );
    assert!(history.last().unwrap().revision > reply.revision);
    assert!(r
        .claim_deliveries("telegram:p:a", "worker-c", now + 60.0, 30.0, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn retry_and_terminal_failure_are_bounded_and_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mailbox.sqlite");
    let conn = open(&path).unwrap();
    conn.execute_batch("INSERT INTO projects(id,name) VALUES ('p','P'); INSERT INTO agents(id,project_id,role,runtime) VALUES ('p:a','p','worker','fake');").unwrap();
    let r = Repository::new(&conn);
    let input = r.ingest(inbound("peer", "42")).unwrap();
    r.reply("p:a", None, input.mailbox_id, "reply", "text", None, None)
        .unwrap();
    let now = super::repository::now();
    let delivery = r
        .claim_deliveries("telegram:p:a", "a", now, 30.0, 1)
        .unwrap()
        .remove(0);
    r.mark_failed(delivery.id, "a", "disconnected", true)
        .unwrap();
    assert!(r
        .claim_deliveries("telegram:p:a", "b", now, 30.0, 1)
        .unwrap()
        .is_empty());
    drop(conn);
    let conn = open(&path).unwrap();
    let r = Repository::new(&conn);
    let retry = r
        .claim_deliveries("telegram:p:a", "b", now + 3.0, 30.0, 1)
        .unwrap()
        .remove(0);
    assert_eq!(retry.id, delivery.id);
    r.mark_failed(retry.id, "b", "unsupported_kind", false)
        .unwrap();
    assert!(r
        .claim_deliveries("telegram:p:a", "c", now + 600.0, 30.0, 1)
        .unwrap()
        .is_empty());
    assert_eq!(
        r.history(&input.conversation_id, None, None, 10)
            .unwrap()
            .last()
            .unwrap()
            .status,
        DeliveryStatus::Failed
    );
}

#[test]
fn repeatedly_crashed_workers_do_not_retry_forever() {
    let conn = connection();
    let r = Repository::new(&conn);
    let input = r.ingest(inbound("peer", "42")).unwrap();
    r.reply("p:a", None, input.mailbox_id, "reply", "text", None, None)
        .unwrap();
    let now = super::repository::now();
    for attempt in 0..10 {
        assert_eq!(
            r.claim_deliveries(
                "telegram:p:a",
                &attempt.to_string(),
                now + f64::from(attempt),
                1.0,
                1
            )
            .unwrap()
            .len(),
            1
        );
    }
    assert!(r
        .claim_deliveries("telegram:p:a", "last", now + 11.0, 1.0, 1)
        .unwrap()
        .is_empty());
    let last = r
        .history(&input.conversation_id, None, None, 10)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(last.status, DeliveryStatus::Failed);
    assert_eq!(last.error.as_deref(), Some("delivery retry limit reached"));
}

#[test]
fn concurrent_ingress_and_delivery_claims_have_single_owners() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mailbox.sqlite");
    let conn = open(&path).unwrap();
    conn.execute_batch("INSERT INTO projects(id,name) VALUES ('p','P'); INSERT INTO agents(id,project_id,role,runtime) VALUES ('p:a','p','worker','fake');").unwrap();
    let handles = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let conn = open(path).unwrap();
                Repository::new(&conn)
                    .ingest(inbound("peer", "source-1"))
                    .unwrap()
                    .id
            })
        })
        .collect::<Vec<_>>();
    let ids = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.iter().all(|id| *id == ids[0]));
    let r = Repository::new(&conn);
    let message = r
        .history(&r.list_conversations(1).unwrap()[0].id, None, None, 1)
        .unwrap()
        .remove(0);
    r.reply("p:a", None, message.mailbox_id, "reply", "text", None, None)
        .unwrap();
    let handles = (0..8)
        .map(|index| {
            let path = path.clone();
            std::thread::spawn(move || {
                let conn = open(path).unwrap();
                Repository::new(&conn)
                    .claim_deliveries(
                        "telegram:p:a",
                        &format!("owner-{index}"),
                        super::repository::now(),
                        60.0,
                        1,
                    )
                    .unwrap()
                    .len()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum::<usize>(),
        1
    );
    assert_eq!(count(&conn, "messages"), 1);
}
