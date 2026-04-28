use axum::{extract::State, http::StatusCode, response::Json, routing::post, Router};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub terminal_id: String,
    pub transcript_path: Option<String>,
}

#[derive(Deserialize)]
struct HookPayload {
    #[serde(rename = "type")]
    event_type: String,
    terminal_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct HookResponse {
    ok: bool,
}

pub async fn run_hook_server_on_listener(
    listener: TcpListener,
    mut shutdown: broadcast::Receiver<()>,
    relay_tx: mpsc::Sender<HookEvent>,
) {
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(relay_tx);

    let addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());

    tracing::info!(target: "cduo::hook", "server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
        })
        .await
        .unwrap_or_else(|e| tracing::error!(target: "cduo::hook", "server error: {e}"));
}

async fn handle_hook(
    State(relay_tx): State<mpsc::Sender<HookEvent>>,
    body: Result<Json<HookPayload>, axum::extract::rejection::JsonRejection>,
) -> (StatusCode, Json<HookResponse>) {
    let payload = match body {
        Ok(Json(p)) => p,
        Err(_) => return (StatusCode::OK, Json(HookResponse { ok: false })),
    };

    if !payload.event_type.eq_ignore_ascii_case("stop")
        || (payload.terminal_id != "a" && payload.terminal_id != "b")
    {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    let event = HookEvent {
        terminal_id: payload.terminal_id,
        transcript_path: payload.transcript_path.filter(|path| !path.is_empty()),
    };

    if relay_tx.try_send(event).is_err() {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    (StatusCode::OK, Json(HookResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::body::Body;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use tower::util::ServiceExt;

    fn make_app() -> (Router, mpsc::Receiver<HookEvent>) {
        let (tx, rx) = mpsc::channel::<HookEvent>(16);
        let app = Router::new()
            .route("/hook", post(handle_hook))
            .with_state(tx);
        (app, rx)
    }

    #[tokio::test]
    async fn test_valid_stop_event() {
        let (app, mut rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop","terminal_id":"a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(resp.ok);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.terminal_id, "a");
    }

    #[tokio::test]
    async fn test_valid_stop_event_pane_b() {
        let (app, mut rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop","terminal_id":"b"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(resp.ok);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.terminal_id, "b");
    }

    #[tokio::test]
    async fn test_invalid_terminal_id() {
        let (app, _rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop","terminal_id":"c"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn test_invalid_event_type() {
        let (app, _rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"start","terminal_id":"a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn test_malformed_json() {
        let (app, _rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn test_missing_fields() {
        let (app, _rx) = make_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn test_event_sent_through_channel() {
        let (tx, mut rx) = mpsc::channel::<HookEvent>(16);
        let app = Router::new()
            .route("/hook", post(handle_hook))
            .with_state(tx);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop","terminal_id":"a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(resp.ok);

        let event = rx.recv().await.unwrap();
        assert_eq!(event.terminal_id, "a");
    }

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
            .with_state(relay_tx);
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
    async fn bound_hook_server_accepts_requests_without_rebinding() {
        let (relay_tx, mut rx) = mpsc::channel::<HookEvent>(8);
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            run_hook_server_on_listener(listener, shutdown_rx, relay_tx).await;
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

    #[tokio::test]
    async fn test_full_channel_returns_without_blocking() {
        let (tx, mut rx) = mpsc::channel::<HookEvent>(1);
        tx.try_send(HookEvent {
            terminal_id: "a".to_string(),
            transcript_path: None,
        })
        .unwrap();
        let app = Router::new()
            .route("/hook", post(handle_hook))
            .with_state(tx);

        let response = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            app.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/hook")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"stop","terminal_id":"a"}"#))
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let resp: HookResponse = serde_json::from_slice(&body).unwrap();
        assert!(!resp.ok);

        let event = rx.recv().await.unwrap();
        assert_eq!(event.terminal_id, "a");
    }
}
