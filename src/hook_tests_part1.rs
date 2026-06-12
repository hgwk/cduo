    use axum::body::to_bytes;
    use axum::body::Body;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use tower::util::ServiceExt;

    fn make_app() -> (Router, mpsc::Receiver<HookEvent>) {
        let (tx, rx) = mpsc::channel::<HookEvent>(16);
        let state = HookState {
            relay_tx: tx,
            ping_tx: None,
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
