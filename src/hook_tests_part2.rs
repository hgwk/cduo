use super::*;
use axum::body::Body;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tower::util::ServiceExt;
#[tokio::test]
async fn test_transcript_path_payload_variants() {
    let (app, mut rx) = make_app();

    for (body, expected) in [
        (
            r#"{"type":"stop","terminal_id":"a","transcript_path":"/tmp/claude.jsonl"}"#,
            Some("/tmp/claude.jsonl"),
        ),
        (
            r#"{"type":"stop","terminal_id":"a","transcript_path":""}"#,
            None,
        ),
        (r#"{"type":"stop","terminal_id":"a"}"#, None),
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.terminal_id, "a");
        assert_eq!(event.transcript_path.as_deref(), expected);
    }
}

#[tokio::test]
async fn template_stop_hook_command_posts_to_http_server() {
    let (relay_tx, mut rx) = mpsc::channel::<HookEvent>(8);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(HookState {
            relay_tx,
            ping_tx: None,
        });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let template: serde_json::Value =
        serde_json::from_str(include_str!("../templates/claude-settings.json")).unwrap();
    let command = template["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();

    let command = command.to_string();
    let status = tokio::task::spawn_blocking(move || {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("ORCHESTRATION_PORT", port.to_string())
            .env("TERMINAL_ID", "b")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(br#"{"transcript_path":"/tmp/from-template.jsonl"}"#)
            .unwrap();
        drop(stdin);
        child.wait().unwrap()
    })
    .await
    .unwrap();
    assert!(status.success());

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    server.abort();

    assert_eq!(event.terminal_id, "b");
    assert_eq!(
        event.transcript_path.as_deref(),
        Some("/tmp/from-template.jsonl")
    );
}

#[tokio::test]
async fn template_stop_hook_active_exits_without_posting() {
    let (relay_tx, mut rx) = mpsc::channel::<HookEvent>(8);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(HookState {
            relay_tx,
            ping_tx: None,
        });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let template: serde_json::Value =
        serde_json::from_str(include_str!("../templates/claude-settings.json")).unwrap();
    let command = template["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();

    let status = tokio::task::spawn_blocking(move || {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("ORCHESTRATION_PORT", port.to_string())
            .env("TERMINAL_ID", "b")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(br#"{"stop_hook_active":true,"transcript_path":"/tmp/skipped.jsonl"}"#)
            .unwrap();
        drop(stdin);
        child.wait().unwrap()
    })
    .await
    .unwrap();

    assert!(status.success());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), rx.recv())
            .await
            .is_err(),
        "active stop hook should not post another cduo hook event"
    );
    server.abort();
}

#[tokio::test]
async fn bound_hook_server_accepts_requests_without_rebinding() {
    let (relay_tx, mut rx) = mpsc::channel::<HookEvent>(8);
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        run_hook_server_on_listener(listener, shutdown_rx, relay_tx, None).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let body = {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request_body = r#"{"type":"stop","terminal_id":"a"}"#;
        let request = format!(
                "POST /hook HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{request_body}",
                request_body.len()
            );
        tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut response)
            .await
            .unwrap();
        response
    };
    assert!(body.starts_with("HTTP/1.1 200 OK"));
    assert!(body.ends_with(r#"{"ok":true}"#));

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.terminal_id, "a");

    let _ = shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}
