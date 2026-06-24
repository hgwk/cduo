use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn communication_gate_route_off_blocks_b_to_a_in_run_loop() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let transcript_path = temp.path().join("claude-route-off-b.jsonl");
    write_claude_transcript(&transcript_path, "ROUTE_OFF_B_TO_A_SHOULD_NOT_SEND");

    let pane_agents = HashMap::from([
        ("a".to_string(), "codex".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);

    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: std::env::current_dir().unwrap(),
        pair_id: "test-pair".to_string(),
        started_at: chrono::Utc::now(),
        log_path,
        pane_agents,
        hook_rx,
        control_rx,
        input_rx,
        write_tx,
        status_tx: None,
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    control_tx
        .send(RelayControl::SetRoute {
            source: "b".to_string(),
            target: "a".to_string(),
            enabled: false,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    hook_tx
        .send(HookEvent {
            terminal_id: "b".to_string(),
            pair_id: None,
            transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_millis(900)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    assert!(writes.is_empty(), "disabled B->A route should not write");
}

#[tokio::test]
async fn communication_gate_codex_to_codex() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("27")
        .join("rollout-codex-to-codex.jsonl");
    let prompt = "COMM_GATE_CODEX_CODEX_PROMPT";
    let answer = "COMM_GATE_CODEX_TO_CODEX";
    write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

    let pane_agents = HashMap::from([
        ("a".to_string(), "codex".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);

    let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: cwd.clone(),
        pair_id: "test-pair".to_string(),
        started_at,
        log_path: temp.path().join("relay.log"),
        pane_agents,
        hook_rx,
        control_rx,
        input_rx,
        write_tx,
        status_tx: None,
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    input_tx
        .send(("a".to_string(), prompt.to_string()))
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    restore_codex_home(prev_codex_home);

    assert_relay_writes(&writes, "b", answer);
}

#[tokio::test]
async fn communication_gate_codex_resume_session_with_old_session_timestamp() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let old_session_ts = chrono::Utc::now() - chrono::Duration::days(1);
    let message_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("27")
        .join("rollout-resumed-codex.jsonl");
    let prompt = "RESUMED_CODEX_PROMPT";
    let answer = "RESUMED_CODEX_ANSWER";
    write_codex_rollout_with_message_timestamp(
        &rollout,
        &cwd,
        old_session_ts,
        message_ts,
        prompt,
        answer,
    );

    let pane_agents = HashMap::from([
        ("a".to_string(), "codex".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);

    let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: cwd.clone(),
        pair_id: "test-pair".to_string(),
        started_at,
        log_path: temp.path().join("relay.log"),
        pane_agents,
        hook_rx,
        control_rx,
        input_rx,
        write_tx,
        status_tx: None,
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    input_tx
        .send(("a".to_string(), prompt.to_string()))
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    restore_codex_home(prev_codex_home);

    assert_relay_writes(&writes, "b", answer);
}
