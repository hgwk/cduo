use super::*;
use axum::body::to_bytes;
use axum::body::Body;
use tower::util::ServiceExt;

fn make_app() -> (Router, mpsc::Receiver<HookEvent>) {
    let (tx, rx) = mpsc::channel::<HookEvent>(16);
    let state = HookState {
        relay_tx: tx,
        ping_tx: None,
        expected_pair_id: None,
    };
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(state);
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
        .with_state(HookState {
            relay_tx: tx,
            ping_tx: None,
            expected_pair_id: None,
        });

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
async fn test_pair_mismatch_is_ignored() {
    let (tx, mut rx) = mpsc::channel::<HookEvent>(16);
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(HookState {
            relay_tx: tx,
            ping_tx: None,
            expected_pair_id: Some("pair-a".to_string()),
        });

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/hook")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"type":"stop","terminal_id":"a","pair_id":"pair-b"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let resp: HookResponse = serde_json::from_slice(&body).unwrap();
    assert!(!resp.ok);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn test_matching_pair_id_is_accepted_when_pair_is_expected() {
    let (tx, mut rx) = mpsc::channel::<HookEvent>(16);
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(HookState {
            relay_tx: tx,
            ping_tx: None,
            expected_pair_id: Some("pair-a".to_string()),
        });

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/hook")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"type":"stop","terminal_id":"a","pair_id":"pair-a"}"#,
                ))
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
    assert_eq!(event.pair_id.as_deref(), Some("pair-a"));
}

#[tokio::test]
async fn test_missing_pair_id_is_ignored_when_pair_is_expected() {
    let (tx, mut rx) = mpsc::channel::<HookEvent>(16);
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(HookState {
            relay_tx: tx,
            ping_tx: None,
            expected_pair_id: Some("pair-a".to_string()),
        });

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
    assert!(!resp.ok);
    assert!(rx.try_recv().is_err());
}
