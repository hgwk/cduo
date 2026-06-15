use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn relay_delivery_prefixes_source_agent_name_in_both_directions() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);
    let mut bus = MessageBus::new();
    let mut rx_a = bus.subscribe("a");
    let mut rx_b = bus.subscribe("b");
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(8);
    let mut pending_prompts = HashMap::new();

    assert_eq!(
        bus.publish(Message::new_relay("a", "b", "FROM_CLAUDE")),
        PublishResult::Delivered
    );
    assert_eq!(
        bus.publish(Message::new_relay("b", "a", "FROM_CODEX")),
        PublishResult::Delivered
    );

    deliver_via_channel(
        &log_path,
        &mut rx_a,
        &mut rx_b,
        &write_tx,
        &pane_agents,
        &mut pending_prompts,
    )
    .await;

    let writes = collect_writes(&mut write_rx, Duration::from_secs(1)).await;
    let writes_a: Vec<(String, Vec<u8>)> = writes
        .iter()
        .filter(|(target, _)| target == "a")
        .cloned()
        .collect();
    let writes_b: Vec<(String, Vec<u8>)> = writes
        .iter()
        .filter(|(target, _)| target == "b")
        .cloned()
        .collect();
    assert_paste_write_contains(&writes_a, "a", "Other Codex says: FROM_CODEX");
    assert_paste_write_contains(&writes_b, "b", "Other Claude says: FROM_CLAUDE");
    assert_eq!(
        pending_prompts.get("a").map(String::as_str),
        Some("Other Codex says: FROM_CODEX")
    );
    assert_eq!(
        pending_prompts.get("b").map(String::as_str),
        Some("Other Claude says: FROM_CLAUDE")
    );
}

#[tokio::test]
async fn relay_delivery_schedules_enter_without_blocking_next_paste() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);
    let mut bus = MessageBus::new();
    let mut rx_a = bus.subscribe("a");
    let mut rx_b = bus.subscribe("b");
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(8);
    let mut pending_prompts = HashMap::new();

    assert_eq!(
        bus.publish(Message::new_relay("test", "a", "FIRST_DELAYED_PASTE")),
        PublishResult::Delivered
    );
    assert_eq!(
        bus.publish(Message::new_relay("test", "b", "SECOND_DELAYED_PASTE")),
        PublishResult::Delivered
    );

    deliver_via_channel(
        &log_path,
        &mut rx_a,
        &mut rx_b,
        &write_tx,
        &pane_agents,
        &mut pending_prompts,
    )
    .await;

    let mut writes_before_enter = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    while tokio::time::Instant::now() < deadline {
        match timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            write_rx.recv(),
        )
        .await
        {
            Ok(Some(item)) => writes_before_enter.push(item),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    writes_before_enter.extend(drain_writes(&mut write_rx));
    let paste_count = writes_before_enter
        .iter()
        .filter(|(_, bytes)| String::from_utf8_lossy(bytes).contains("\x1b[200~"))
        .count();
    assert_eq!(
        paste_count, 2,
        "both paste bundles should be queued before Claude's submit delay elapses"
    );
    assert!(
        writes_before_enter.iter().all(|(_, bytes)| bytes != b"\r"),
        "Enter should remain delayed instead of blocking the relay drain"
    );

    let writes_after_delay = collect_writes(&mut write_rx, Duration::from_secs(2)).await;
    let enter_count = writes_after_delay
        .iter()
        .filter(|(_, bytes)| bytes == b"\r")
        .count();
    assert_eq!(enter_count, 2, "each delayed paste should still submit");
}

#[test]
fn delivered_content_can_be_prefixed_before_publish() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let output = transcript_output("PREFIXED_DELIVERY_BODY");
    let mut controls = RelayControlState::default();
    controls.set_delivery_prefix("[relay from peer] ");

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &output,
        &mut controls,
    ));
    let msg = rx_b.try_recv().expect("prefixed delivery should publish");
    assert_eq!(msg.source_node_id, "a");
    assert_eq!(msg.target_node_id, "b");
    assert_eq!(msg.content, "[relay from peer] PREFIXED_DELIVERY_BODY");
    assert_eq!(output.output, "PREFIXED_DELIVERY_BODY");
}

#[test]
fn relay_stop_marker_blocks_publish_and_stops_future_auto_relay() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState::default();

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("done CDUO_STOP_RELAY"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_err());

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
fn explicit_stop_token_only_stops_when_returned_exactly() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let mut controls = RelayControlState::default();

    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("Here is a fenced block:\n~~~\nbody\n~~~"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_ok());

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("~~~"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_err());

    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &transcript_output("OUTPUT_AFTER_EXPLICIT_STOP"),
        &mut controls,
    ));
    assert!(rx_b.try_recv().is_err());
}
