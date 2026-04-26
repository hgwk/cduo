use axum::{extract::State, http::StatusCode, response::Json, routing::post, Router};
use serde::Deserialize;
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

pub async fn run_hook_server(
    port: u16,
    mut shutdown: broadcast::Receiver<()>,
    relay_tx: mpsc::Sender<HookEvent>,
) {
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(relay_tx);

    let addr = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[hook] Failed to bind to {addr}: {e}");
            return;
        }
    };

    println!("[hook] Server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
        })
        .await
        .unwrap_or_else(|e| eprintln!("[hook] Server error: {e}"));
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
