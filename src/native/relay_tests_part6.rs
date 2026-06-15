use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn communication_gate_claude_b_to_codex_a_without_hook_transcript_path() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let claude_home = temp.path().join("claude");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_claude_home = std::env::var_os("CLAUDE_HOME");
    std::env::set_var("CLAUDE_HOME", &claude_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let transcript_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let transcript_path = claude_home
        .join("projects")
        .join("-tmp-project")
        .join("claude-b-fallback.jsonl");
    let answer = "COMM_GATE_CLAUDE_B_TO_CODEX_A_FALLBACK";
    write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

    let pane_agents = HashMap::from([
        ("a".to_string(), "codex".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);

    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: cwd.clone(),
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

    hook_tx
        .send(HookEvent {
            terminal_id: "b".to_string(),
            transcript_path: None,
        })
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    restore_claude_home(prev_claude_home);

    assert_relay_writes(&writes, "a", answer);
}

#[tokio::test]
async fn communication_gate_claude_b_to_claude_a_without_hook_transcript_path() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let claude_home = temp.path().join("claude");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_claude_home = std::env::var_os("CLAUDE_HOME");
    std::env::set_var("CLAUDE_HOME", &claude_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let transcript_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let transcript_path = claude_home
        .join("projects")
        .join("-tmp-project")
        .join("claude-b-to-claude-a-fallback.jsonl");
    let answer = "COMM_GATE_CLAUDE_B_TO_CLAUDE_A_FALLBACK";
    write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "claude".to_string()),
    ]);

    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(run(RelayInputs {
        cwd: cwd.clone(),
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

    hook_tx
        .send(HookEvent {
            terminal_id: "b".to_string(),
            transcript_path: None,
        })
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    restore_claude_home(prev_claude_home);

    assert_relay_writes(&writes, "a", answer);
}

#[tokio::test]
async fn communication_gate_route_off_blocks_a_to_b_in_run_loop() {
    let temp = tempdir().unwrap();
    let log_path = temp.path().join("relay.log");
    let transcript_path = temp.path().join("claude-route-off.jsonl");
    write_claude_transcript(&transcript_path, "ROUTE_OFF_A_TO_B_SHOULD_NOT_SEND");

    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);

    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
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
        status_tx: None,
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    control_tx
        .send(RelayControl::SetRoute {
            source: "a".to_string(),
            target: "b".to_string(),
            enabled: false,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    hook_tx
        .send(HookEvent {
            terminal_id: "a".to_string(),
            transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();

    let writes = collect_writes(&mut write_rx, Duration::from_millis(900)).await;
    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    assert!(writes.is_empty(), "disabled A->B route should not write");
}
