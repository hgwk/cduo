use super::*;
use tempfile::tempdir;
#[test]
fn relay_route_control_disables_and_enables_a_to_b() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_b = bus.subscribe("b");
    let output = transcript_output("CONTROLLED_A_TO_B_OUTPUT");
    let mut controls = RelayControlState::default();

    assert!(controls.set_route_enabled("a", "b", false));
    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &output,
        &mut controls,
    ));
    assert!(
        rx_b.try_recv().is_err(),
        "disabled A->B route should not deliver"
    );

    assert!(controls.set_route_enabled("a", "b", true));
    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "a",
        &output,
        &mut controls,
    ));
    let msg = rx_b.try_recv().expect("enabled A->B route should deliver");
    assert_eq!(msg.source_node_id, "a");
    assert_eq!(msg.target_node_id, "b");
    assert_eq!(msg.content, output.output);
}

#[test]
fn relay_route_control_disables_and_enables_b_to_a() {
    let temp = tempdir().unwrap();
    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_a = bus.subscribe("a");
    let output = transcript_output("CONTROLLED_B_TO_A_OUTPUT");
    let mut controls = RelayControlState::default();

    assert!(controls.set_route_enabled("b", "a", false));
    assert!(!publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &output,
        &mut controls,
    ));
    assert!(
        rx_a.try_recv().is_err(),
        "disabled B->A route should not deliver"
    );

    assert!(controls.set_route_enabled("b", "a", true));
    assert!(publish_transcript_output_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &output,
        &mut controls,
    ));
    let msg = rx_a.try_recv().expect("enabled B->A route should deliver");
    assert_eq!(msg.source_node_id, "b");
    assert_eq!(msg.target_node_id, "a");
    assert_eq!(msg.content, output.output);
}

#[test]
fn manual_relay_request_reads_bound_codex_transcript() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let rollout = temp.path().join("rollout-bound.jsonl");
    let answer = "MANUAL_BOUND_CODEX_RESPONSE";
    write_codex_rollout(
        &rollout,
        &cwd,
        chrono::Utc::now(),
        "BOUND_CODEX_PROMPT",
        answer,
    );

    let router = PairRouter::new("a", "b");
    let mut bus = MessageBus::new();
    let mut rx_a = bus.subscribe("a");
    let transcripts = HashMap::from([("b".to_string(), rollout)]);
    let mut last_signatures = HashMap::new();
    let mut controls = RelayControlState::default();

    assert!(publish_bound_codex_transcript_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &transcripts,
        &mut last_signatures,
        &mut controls,
    ));
    let msg = rx_a
        .try_recv()
        .expect("manual request should deliver from bound Codex rollout");
    assert_eq!(msg.source_node_id, "b");
    assert_eq!(msg.target_node_id, "a");
    assert_eq!(msg.content, answer);

    assert!(!publish_bound_codex_transcript_with_controls(
        &mut bus,
        &router,
        &temp.path().join("relay.log"),
        "b",
        &transcripts,
        &mut last_signatures,
        &mut controls,
    ));
    assert!(
        rx_a.try_recv().is_err(),
        "manual relay should deduplicate the already delivered transcript output"
    );
}

#[tokio::test]
async fn manual_relay_primes_codex_target_prompt_binding() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let transcript_path = temp.path().join("claude.jsonl");
    let answer = "MANUAL_CLAUDE_TO_CODEX_PROMPT";
    write_claude_transcript(&transcript_path, answer);

    let router = PairRouter::new("a", "b");
    let controls = RelayControlState::default();
    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);
    let codex_transcripts = HashMap::new();
    let claude_transcripts = HashMap::from([("a".to_string(), transcript_path.to_path_buf())]);
    let mut pending_prompts = HashMap::new();
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(8);

    manual_relay(
        "a",
        ManualRelayContext {
            router: &router,
            controls: &controls,
            pane_agents: &pane_agents,
            codex_transcripts: &codex_transcripts,
            claude_transcripts: &claude_transcripts,
            pending_prompts: &mut pending_prompts,
            write_tx: &write_tx,
            log_path: &log_path,
        },
    )
    .await;

    assert_eq!(
        pending_prompts.get("b").map(|prompt| prompt.text.as_str()),
        Some("Other Claude says: MANUAL_CLAUDE_TO_CODEX_PROMPT"),
        "manual relay should prime Codex transcript binding for the target pane"
    );
    let writes = collect_writes(&mut write_rx, Duration::from_secs(1)).await;
    assert_relay_writes(
        &writes,
        "b",
        "Other Claude says: MANUAL_CLAUDE_TO_CODEX_PROMPT",
    );
}
