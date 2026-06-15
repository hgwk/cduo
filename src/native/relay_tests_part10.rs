use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn codex_rebinds_when_next_prompt_appears_in_new_rollout() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let first_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let second_ts = chrono::Utc::now() + chrono::Duration::seconds(2);
    let first_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("27")
        .join("rollout-first.jsonl");
    let second_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("27")
        .join("rollout-second.jsonl");
    let first_prompt = "FIRST_ROLLOUT_PROMPT";
    let first_answer = "FIRST_ROLLOUT_ANSWER";
    write_codex_rollout(&first_rollout, &cwd, first_ts, first_prompt, first_answer);

    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);

    let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
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
        .send(("b".to_string(), first_prompt.to_string()))
        .await
        .unwrap();
    let first_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
    assert!(
        first_writes
            .iter()
            .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(first_answer)),
        "expected first rollout answer to relay"
    );

    let second_prompt = "SECOND_ROLLOUT_PROMPT";
    let second_answer = "SECOND_ROLLOUT_ANSWER";
    write_codex_rollout(
        &second_rollout,
        &cwd,
        second_ts,
        second_prompt,
        second_answer,
    );
    input_tx
        .send(("b".to_string(), second_prompt.to_string()))
        .await
        .unwrap();
    let second_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;

    let _ = shutdown_tx.send(());
    let _ = timeout(Duration::from_secs(2), handle).await;

    if let Some(prev) = prev_codex_home {
        std::env::set_var("CODEX_HOME", prev);
    } else {
        std::env::remove_var("CODEX_HOME");
    }

    assert!(
        second_writes.iter().any(|(target, bytes)| target == "a"
            && String::from_utf8_lossy(bytes).contains(second_answer)),
        "expected Codex pane to rebind to the rollout containing the latest prompt"
    );
}
