use super::*;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

fn state_at(path: &Path) -> State {
    State {
        conn: Mutex::new(open_mailbox(path).unwrap()),
        allow: vec![7],
        manager: Some("p:manager".into()),
        manager_display_name: None,
        tmux_prefix: "t-".into(),
        media_root: path.parent().unwrap().join("media"),
        stt: None,
        typing: Mutex::new(HashMap::new()),
        rich: false,
    }
}

/// A local Telegram API substitute exercises the production sender. It
/// records each request and returns the supplied success/failure sequence.
async fn fake_bot(successes: Vec<bool>) -> (Bot, tokio::task::JoinHandle<Vec<Value>>) {
    fake_bot_with_gate(successes, None).await
}

async fn fake_bot_with_gate(
    successes: Vec<bool>,
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
) -> (Bot, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (index, success) in successes.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            let header_end = loop {
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
            let lower_headers = headers.to_lowercase();
            let length: usize = lower_headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while bytes.len() < header_end + length {
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            requests.push(request_body(
                &bytes[header_end..header_end + length],
                &headers,
            ));
            if index == 0 {
                if let Some((received, release)) = &gate {
                    received.notify_one();
                    release.notified().await;
                }
            }
            let body = if success {
                if lower_headers.contains("/deletemessage ") {
                    json!({"ok":true,"result":true})
                } else {
                    json!({"ok":true,"result":{"message_id":1,"date":0,"chat":{"id":7,"type":"private"},"text":"delivered"}})
                }
            } else {
                json!({"ok":false,"error_code":500,"description":"temporary failure"})
            }.to_string();
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
        }
        requests
    });
    (
        Bot::new("123:test-token").set_api_url(url.parse().unwrap()),
        task,
    )
}

fn request_body(bytes: &[u8], headers: &str) -> Value {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return value;
    }
    let content_type = headers
        .lines()
        .find(|line| line.to_lowercase().starts_with("content-type:"))
        .unwrap();
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .unwrap()
        .trim()
        .trim_matches('"');
    let mut value = serde_json::Map::new();
    for part in String::from_utf8_lossy(bytes).split(&format!("--{boundary}")) {
        if let Some((metadata, body)) = part.split_once("\r\n\r\n") {
            let name = metadata
                .split("name=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            value.insert(
                name.into(),
                Value::String(body.strip_suffix("\r\n").unwrap_or(body).into()),
            );
        }
    }
    Value::Object(value)
}

fn inbound(text: &str) -> Message {
    serde_json::from_value(
        json!({"message_id":21,"date":0,"chat":{"id":7,"type":"private"},"text":text}),
    )
    .unwrap()
}

#[tokio::test]
async fn expiry_during_approval_transport_removes_the_late_card_without_marking_delivery() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state_at(&dir.path().join("mailbox.sqlite")));
    state.conn.lock().await.execute(
        "INSERT INTO approvals (project_id,agent_id,action,summary,status,requested_at,expires_at)
         VALUES ('p','p:manager','deploy','delayed card','pending',0,strftime('%s','now')+60)", [],
    ).unwrap();
    let received = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (bot, server) =
        fake_bot_with_gate(vec![true, true], Some((received.clone(), release.clone()))).await;
    let task_state = state.clone();
    let send = tokio::spawn(async move { outbound_once(&bot, &task_state, ChatId(7)).await });
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .unwrap();
    {
        let conn = state.conn.lock().await;
        // A concurrent expiry sweep wins while Telegram is accepting the send.
        conn.execute("UPDATE approvals SET expires_at=0", [])
            .unwrap();
        conn.execute("UPDATE approvals SET status='undeliverable' WHERE status='pending' AND delivered_at IS NULL AND expires_at < strftime('%s','now')", []).unwrap();
    }
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), send)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["message_id"], 1, "late card was deleted");
    let conn = state.conn.lock().await;
    let (status, delivered_at): (String, Option<f64>) = conn
        .query_row("SELECT status, delivered_at FROM approvals", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(status, "undeliverable");
    assert!(delivered_at.is_none());
}

#[tokio::test]
async fn public_text_and_caption_limits_preserve_the_full_body_without_attribution_overflow() {
    let cases = [
        (None, "a".repeat(4096), false),
        (None, "🦀".repeat(2048), false),
        (None, "🦀".repeat(2048), true),
        (Some("image"), "a".repeat(1024), false),
        (Some("file"), "🦀".repeat(512), false),
    ];
    let (bot, server) = fake_bot(vec![true; cases.len()]).await;
    for (kind, body, rich) in &cases {
        let row = MailboxRow {
            id: 1, sender: "p:manager".into(), text: body.clone(),
            kind: kind.map(str::to_string), telegram_msg_id: None,
            payload: kind.map(|_| json!({"source":"url", "value":"https://example.invalid/file.png", "caption":body}).to_string()),
        };
        forward_row(&bot, ChatId(7), &row, *rich).await.unwrap();
    }
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    for ((kind, body, rich), request) in cases.iter().zip(requests.iter()) {
        let sent = if kind.is_some() {
            &request["caption"]
        } else if *rich {
            &request["rich_message"]["markdown"]
        } else {
            &request["text"]
        };
        assert_eq!(sent.as_str().unwrap(), body);
        assert_eq!(
            sent.as_str().unwrap().encode_utf16().count(),
            if kind.is_some() { 1024 } else { 4096 }
        );
    }
}

#[tokio::test]
async fn oversized_cli_text_is_sent_in_bounded_unicode_chunks() {
    let body = "🦀".repeat(4097);
    let row = MailboxRow {
        id: 1,
        sender: "p:manager".into(),
        text: body.clone(),
        kind: None,
        payload: None,
        telegram_msg_id: Some(21),
    };
    let (bot, server) = fake_bot(vec![true; 3]).await;
    forward_row(&bot, ChatId(7), &row, false).await.unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let mut reconstructed = String::new();
    for request in requests {
        let text = request["text"].as_str().unwrap();
        assert!(text.encode_utf16().count() <= 4096);
        assert_eq!(request["reply_parameters"]["message_id"], 21);
        reconstructed.push_str(
            text.strip_suffix("\n\n— replied by p:manager")
                .unwrap_or(text),
        );
    }
    assert_eq!(reconstructed, body);
}

#[tokio::test]
async fn a_final_media_flush_failure_is_returned_to_the_error_path() {
    struct FailingFlush;
    impl tokio::io::AsyncWrite for FailingFlush {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("disk full")))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let error = flush_download(&mut FailingFlush).await.unwrap_err();
    assert_eq!(error, "flush downloaded media: disk full");
    let payload: Value = serde_json::from_str(&media_error_payload("caption", &error)).unwrap();
    assert_eq!(payload["error"], error);
}

#[test]
fn malformed_allowlist_fails_and_empty_configuration_authorizes_no_chat() {
    assert!(parse_authorized_chats(Some("7,invalid")).is_err());
    assert_eq!(
        parse_authorized_chats(Some(" 7, -19 ")).unwrap(),
        vec![7, -19]
    );
    assert!(parse_authorized_chats(None).unwrap().is_empty());
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.clear();
    assert!(!state.is_authorized(7));
}

#[tokio::test]
async fn outbound_failures_survive_restart_and_successes_are_consumed_once() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mailbox.sqlite");
    let state = state_at(&path);
    {
        let conn = state.conn.lock().await;
        conn.execute("INSERT INTO messages (project_id,sender,recipient,text,sent_at) VALUES ('p','p:manager','user:telegram','queued reply',0)", []).unwrap();
        conn.execute("INSERT INTO approvals (project_id,agent_id,action,summary,status,requested_at,expires_at) VALUES ('p','p:manager','deploy','queued card','pending',0,strftime('%s','now')+60)", []).unwrap();
    }
    let (bot, server) = fake_bot(vec![false, false, true, true]).await;
    assert!(outbound_once(&bot, &state, ChatId(7)).await.is_err());
    {
        let conn = state.conn.lock().await;
        assert_eq!(outgoing_messages(&conn, Some("p")).unwrap().len(), 1);
        assert_eq!(pending_approvals(&conn, Some("p")).unwrap().len(), 1);
    }
    drop(state);
    let restarted = state_at(&path);
    outbound_once(&bot, &restarted, ChatId(7)).await.unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requests.len(), 4);
    outbound_once(&bot, &restarted, ChatId(7)).await.unwrap();
    let conn = restarted.conn.lock().await;
    assert!(outgoing_messages(&conn, Some("p")).unwrap().is_empty());
    assert!(pending_approvals(&conn, Some("p")).unwrap().is_empty());
}

#[tokio::test]
async fn rich_api_rejection_uses_html_fallback_instead_of_acknowledging_failure() {
    let (bot, server) = fake_bot(vec![false, true]).await;
    let row = MailboxRow {
        id: 1,
        sender: "p:manager".into(),
        text: "hello".into(),
        kind: None,
        payload: None,
        telegram_msg_id: None,
    };
    forward_row(&bot, ChatId(7), &row, true).await.unwrap();
    let requests = server.await.unwrap();
    assert!(requests[0].get("rich_message").is_some());
    assert_eq!(requests[1]["text"], "hello\n\n— replied by p:manager");
}

#[tokio::test]
async fn empty_allowlist_permits_only_start_discovery() {
    let dir = TempDir::new().unwrap();
    let mut state = state_at(&dir.path().join("mailbox.sqlite"));
    state.allow.clear();
    let state = Arc::new(state);
    let (bot, server) = fake_bot(vec![true]).await;
    handle_message(bot.clone(), inbound("arbitrary text"), state.clone())
        .await
        .unwrap();
    handle_message(bot, inbound("/start"), state.clone())
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert!(requests[0]["text"]
        .as_str()
        .unwrap()
        .contains("Your chat id: 7"));
    let conn = state.conn.lock().await;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn failed_inbound_insert_never_reports_successful_routing() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state_at(&dir.path().join("mailbox.sqlite")));
    state.conn.lock().await.execute_batch("CREATE TRIGGER fail_inbound BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL,'test insert failure'); END;").unwrap();
    let (bot, server) = fake_bot(vec![true]).await;
    handle_message(bot, inbound("hello"), state).await.unwrap();
    let requests = server.await.unwrap();
    assert_eq!(
        requests[0]["text"],
        "internal error: could not record the message; please retry."
    );
}

#[test]
fn expired_approval_cannot_be_decided_between_expiry_sweeps() {
    let conn = Connection::open_in_memory().unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    conn.execute("INSERT INTO approvals (project_id,agent_id,action,summary,status,requested_at,expires_at) VALUES ('p','p:manager','deploy','card','pending',0,0)", []).unwrap();
    assert!(!decide_approval(&conn, conn.last_insert_rowid(), "approved", None).unwrap());
    assert!(pending_approvals(&conn, None).unwrap().is_empty());
    let status: String = conn
        .query_row("SELECT status FROM approvals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status, "pending");
}

#[test]
fn media_completion_publishes_a_fresh_id_and_preserves_telegram_threading() {
    let conn = Connection::open_in_memory().unwrap();
    team_core::mailbox::ensure(&conn).unwrap();
    conn.execute("INSERT INTO messages (project_id,sender,recipient,text,sent_at,kind,telegram_msg_id) VALUES ('p','user:telegram','p:manager','caption',0,'media_pending',777)", []).unwrap();
    let staged = conn.last_insert_rowid();
    assert!(lookup_replied_row(&conn, 777).is_none());
    conn.execute("INSERT INTO messages (project_id,sender,recipient,text,sent_at) VALUES ('p','user:telegram','p:manager','later',0)", []).unwrap();
    let high_water = conn.last_insert_rowid();
    let published =
        finalize_media(&conn, staged, "image", "{\"path\":\"/tmp/image.png\"}").unwrap();
    assert!(published > high_water);
    let telegram_id: i64 = conn
        .query_row(
            "SELECT telegram_msg_id FROM messages WHERE id=?1",
            params![published],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(telegram_id, 777);
    assert_eq!(lookup_replied_row(&conn, 777).unwrap().0, published);
    assert!(conn
        .query_row(
            "SELECT id FROM messages WHERE id=?1",
            params![staged],
            |row| row.get::<_, i64>(0)
        )
        .is_err());
}
