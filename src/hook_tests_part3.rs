use super::*;
use axum::body::{to_bytes, Body};
use tower::util::ServiceExt;
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
        .with_state(HookState {
            relay_tx: tx,
            ping_tx: None,
        });

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
