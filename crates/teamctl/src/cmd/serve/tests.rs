use super::*;
use axum::{
    body::to_bytes,
    http::{Method, Request},
};
use tower::ServiceExt;
fn fixture() -> (tempfile::TempDir, Shared) {
    let temp = tempfile::tempdir().unwrap();
    demo::prepare(temp.path()).unwrap();
    let compose = super::super::load(temp.path()).unwrap();
    super::super::up::register_all_public(&compose).unwrap();
    let state = Arc::new(Workspace {
        root: temp.path().into(),
        db: temp.path().join(&compose.global.broker.path),
        assets: temp.path().join("state/web/assets"),
        retention_days: Some(30),
        max_upload: 1024,
        token: "a".repeat(64),
        nonce: "n".repeat(64),
        origin: "http://127.0.0.1:8787".into(),
        authority: "127.0.0.1:8787".into(),
        demo: true,
        shells: Default::default(),
    });
    let c = team_core::chat::open(&state.db).unwrap();
    team_core::interfaces::sync(&c, &compose).unwrap();
    assets::ensure(&state, &c).unwrap();
    demo::seed(&state, &c).unwrap();
    (temp, state)
}
async fn call(state: Shared, method: Method, path: &str, body: Body, auth: bool) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", &state.authority)
        .header("content-type", "application/json");
    if auth {
        request = request.header("authorization", format!("Bearer {}", state.token));
    }
    router(state)
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
#[tokio::test]
async fn auth_host_and_origin_are_enforced() {
    let (_temp, state) = fixture();
    let response = call(
        state.clone(),
        Method::GET,
        "/api/state",
        Body::empty(),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for (header, content) in [
        ("host", "attacker.test"),
        ("origin", "http://attacker.test"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let mut request = Request::builder()
            .uri("/api/state")
            .header("authorization", format!("Bearer {}", state.token));
        if header != "host" {
            request = request.header("host", &state.authority);
        }
        request = request.header(header, content);
        let response = router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = call(state.clone(), Method::GET, "/", Body::empty(), false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
}
#[tokio::test]
async fn conversation_round_trip_is_durable_and_has_provenance() {
    let (_temp, state) = fixture();
    let body =
        json!({"agent":"personal:companion","text":"Please remember this","request_id":"client-1"});
    let response = call(
        state.clone(),
        Method::POST,
        "/api/messages",
        Body::from(body.to_string()),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let message = value(response).await;
    assert_eq!(message["message"]["interface"]["id"], "web");
    let response = call(
        state.clone(),
        Method::GET,
        "/api/messages?agent=personal%3Acompanion",
        Body::empty(),
        true,
    )
    .await;
    let history = value(response).await;
    assert!(history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "Please remember this"));
    let count = history["messages"].as_array().unwrap().len();
    let _ = call(
        state.clone(),
        Method::POST,
        "/api/messages",
        Body::from(body.to_string()),
        true,
    )
    .await;
    let history = value(
        call(
            state,
            Method::GET,
            "/api/messages?agent=personal%3Acompanion",
            Body::empty(),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(history["messages"].as_array().unwrap().len(), count);
}
#[tokio::test]
async fn attachment_auth_limits_expiry_and_unsafe_mime() {
    let (_temp, state) = fixture();
    let request = Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("host", &state.authority)
        .header("authorization", format!("Bearer {}", state.token))
        .header("content-type", "text/html")
        .header("x-file-name", "malicious.html")
        .body(Body::from("<script>alert(1)</script>"))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let item = value(response).await;
    assert_eq!(item["mime_type"], "application/octet-stream");
    let url = item["url"].as_str().unwrap();
    assert_eq!(
        call(state.clone(), Method::GET, url, Body::empty(), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(state.clone(), Method::GET, url, Body::empty(), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(
        call(
            state.clone(),
            Method::POST,
            "/api/assets",
            Body::from(vec![0u8; 1025]),
            true
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let c = team_core::chat::open(&state.db).unwrap();
    assets::cleanup(
        &state,
        &c,
        item["created_at"].as_f64().unwrap() + 31.0 * 86400.0,
    )
    .unwrap();
    assert_eq!(
        call(state, Method::GET, url, Body::empty(), true)
            .await
            .status(),
        StatusCode::GONE
    );
}
#[tokio::test]
async fn approvals_reject_invalid_choices_and_duplicate_decisions() {
    let (_temp, state) = fixture();
    let c = team_core::chat::open(&state.db).unwrap();
    c.execute("UPDATE approvals SET options_json='[{\"label\":\"Later\",\"value\":\"later\"}]' WHERE id=1",[]).unwrap();
    let invalid = call(
        state.clone(),
        Method::POST,
        "/api/approvals/1",
        Body::from(json!({"decision":"approve","value":"injected"}).to_string()),
        true,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let approved = call(
        state.clone(),
        Method::POST,
        "/api/approvals/1",
        Body::from(json!({"decision":"approve","value":"later"}).to_string()),
        true,
    )
    .await;
    assert_eq!(approved.status(), StatusCode::OK);
    let duplicate = call(
        state,
        Method::POST,
        "/api/approvals/1",
        Body::from(json!({"decision":"reject"}).to_string()),
        true,
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
}
#[tokio::test]
async fn unknown_agent_and_terminal_key_are_rejected() {
    let (_temp, state) = fixture();
    assert_eq!(
        call(
            state.clone(),
            Method::GET,
            "/api/messages?agent=missing",
            Body::empty(),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            state,
            Method::POST,
            "/api/agents/personal%3Acompanion/terminal",
            Body::from(json!({"input":"","key":"C-a; evil"}).to_string()),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn state_contains_inspection_without_secret_values() {
    let (_temp, state) = fixture();
    let response = call(state, Method::GET, "/api/state", Body::empty(), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = value(response).await;
    assert_eq!(snapshot["agents"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["cache"]["retention_days"], 30);
    assert!(snapshot.get("token").is_none());
}
#[tokio::test]
async fn channel_retry_is_deduplicated_and_cursor_does_not_skip_backlog() {
    let (_temp, state) = fixture();
    let body = json!({"channel":"personal:notes","text":"one task","request_id":"channel-retry"})
        .to_string();
    let first = value(
        call(
            state.clone(),
            Method::POST,
            "/api/messages",
            Body::from(body.clone()),
            true,
        )
        .await,
    )
    .await;
    let second = value(
        call(
            state.clone(),
            Method::POST,
            "/api/messages",
            Body::from(body),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(first["message"]["id"], second["message"]["id"]);
    let c = team_core::chat::open(&state.db).unwrap();
    for i in 0..220 {
        c.execute("INSERT INTO messages(project_id,sender,recipient,text,kind,sent_at) VALUES('personal','personal:scout','channel:personal:notes',?1,'text',?2)",rusqlite::params![format!("backlog{i}"),now()]).unwrap();
    }
    let history = value(
        call(
            state,
            Method::GET,
            "/api/messages?channel=personal%3Anotes&after=0",
            Body::empty(),
            true,
        )
        .await,
    )
    .await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 200);
    assert_eq!(messages[0]["text"], "one task");
    assert!(messages[1]["interface"].is_null());
}
#[tokio::test]
async fn unicode_filename_round_trips_without_header_injection() {
    let (_temp, state) = fixture();
    let request = Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("host", &state.authority)
        .header("authorization", format!("Bearer {}", state.token))
        .header("x-file-name", "caf%C3%A9%0D%0A.txt")
        .body(Body::from("hello"))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let item = value(response).await;
    assert_eq!(item["name"], "café.txt");
    let response = call(
        state,
        Method::GET,
        item["url"].as_str().unwrap(),
        Body::empty(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .contains("caf%C3%A9.txt"));
}
#[tokio::test]
async fn exited_workspace_shell_is_reported_and_can_reconnect() {
    let (_temp, state) = fixture();
    let endpoint = "/api/agents/personal%3Acompanion/terminal";
    let response = call(
        state.clone(),
        Method::POST,
        endpoint,
        Body::from(json!({"mode":"shell","connect":true}).to_string()),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = call(
        state.clone(),
        Method::POST,
        endpoint,
        Body::from(json!({"mode":"shell","input":"exit","key":"Enter"}).to_string()),
        true,
    )
    .await;
    let mut exited = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let snapshot = value(
            call(
                state.clone(),
                Method::GET,
                &format!("{endpoint}?mode=shell"),
                Body::empty(),
                true,
            )
            .await,
        )
        .await;
        if snapshot["connected"] == false {
            exited = true;
            break;
        }
    }
    assert!(exited, "shell did not report its exit");
    let response = call(
        state.clone(),
        Method::POST,
        endpoint,
        Body::from(json!({"mode":"shell","connect":true}).to_string()),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = value(
        call(
            state,
            Method::GET,
            &format!("{endpoint}?mode=shell"),
            Body::empty(),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(snapshot["connected"], true);
}
