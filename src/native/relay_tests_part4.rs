use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn relay_reports_auto_stopped_status_after_explicit_stop_token() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let transcript_path = temp.path().join("claude-stop.jsonl");
    write_claude_transcript(&transcript_path, "~~~");

    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);

    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, _write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (status_tx, mut status_rx) = mpsc::channel::<RelayStatus>(8);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: std::env::current_dir().unwrap(),
        started_at: chrono::Utc::now(),
        log_path,
        pane_agents,
        hook_rx,
        control_rx,
        input_rx,
        write_tx,
        status_tx: Some(status_tx),
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    let initial = timeout(Duration::from_secs(1), status_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!initial.auto_stopped);

    hook_tx
        .send(HookEvent {
            terminal_id: "a".to_string(),
            transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();

    let stopped = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = status_rx.recv().await {
                if status.auto_stopped {
                    break status;
                }
            }
        }
    })
    .await
    .unwrap();

    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    assert!(stopped.auto_stopped);
}

#[test]
fn reset_stop_reenables_auto_relay_after_duplicate_stop() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState::default();
    let repeated = transcript_output("DUPLICATE_RELAY_BODY");

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &repeated,
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_ok());

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &repeated,
        &mut controls,
    ));
    assert!(controls.stopped);

    controls.reset_stop();
    bus.clear_dedup();

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &repeated,
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_ok());
    assert!(!controls.stopped);
}

#[test]
fn max_relay_turns_blocks_auto_ping_pong_after_limit() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState {
        max_auto_relays: Some(1),
        ..RelayControlState::default()
    };

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("FIRST_ALLOWED_RELAY"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_ok());

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("SECOND_BLOCKED_RELAY"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn duplicate_auto_output_stops_ping_pong_after_first_delivery() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_a = bus.subscribe("a");
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState::default();
    let repeated_output = "REPEATED_STATUS_OUTPUT";

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output(repeated_output),
        &mut controls,
    ));
    assert_eq!(rx_b.try_recv().unwrap().content, repeated_output);

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &transcript_output(repeated_output),
        &mut controls,
    ));
    assert!(rx_a.try_recv().is_err());
    assert!(controls.stopped);

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("NEXT_OUTPUT_SHOULD_NOT_RELAY"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn duplicate_auto_output_from_same_source_does_not_stop_relay() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState::default();
    let repeated_output = "REPEATED_VALID_OUTPUT";

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output(repeated_output),
        &mut controls,
    ));
    assert_eq!(rx_b.try_recv().unwrap().content, repeated_output);

    assert!(
        !publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output(repeated_output),
            &mut controls,
        ),
        "message bus may deduplicate the same route, but relay should stay active"
    );
    assert!(!controls.stopped);

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("NEXT_OUTPUT_SHOULD_RELAY"),
        &mut controls,
    ));
    assert_eq!(rx_b.try_recv().unwrap().content, "NEXT_OUTPUT_SHOULD_RELAY");
}
