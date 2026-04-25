use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub terminal_id: String,
    #[allow(dead_code)]
    pub event_type: String,
}

#[derive(Deserialize)]
struct HookPayload {
    #[serde(rename = "type")]
    event_type: String,
    terminal_id: String,
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

    if payload.event_type != "stop" || (payload.terminal_id != "a" && payload.terminal_id != "b") {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    let event = HookEvent {
        terminal_id: payload.terminal_id,
        event_type: payload.event_type,
    };

    if relay_tx.send(event).await.is_err() {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    (StatusCode::OK, Json(HookResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::body::to_bytes;
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
        assert_eq!(event.event_type, "stop");
    }
}
