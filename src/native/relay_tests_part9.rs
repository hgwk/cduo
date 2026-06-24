use super::*;
use tempfile::tempdir;
#[tokio::test]
async fn codex_transcript_binding_excludes_rollout_bound_to_other_pane() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
    let prompt = "SAME_PROMPT_FOR_TWO_CODEX_PANES";
    let first_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("28")
        .join("rollout-first-pane.jsonl");
    let second_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("04")
        .join("28")
        .join("rollout-second-pane.jsonl");
    write_codex_rollout(
        &first_rollout,
        &cwd,
        session_ts,
        prompt,
        "FIRST_PANE_ANSWER",
    );
    write_codex_rollout(
        &second_rollout,
        &cwd,
        session_ts,
        prompt,
        "SECOND_PANE_ANSWER",
    );

    let mut transcripts = HashMap::from([("b".to_string(), second_rollout.clone())]);
    let pending_prompts =
        HashMap::from([("a".to_string(), PendingPrompt::new(prompt.to_string()))]);

    ensure_codex_transcript_local(
        "a",
        &mut transcripts,
        &pending_prompts,
        &cwd,
        started_at,
        &temp.path().join("relay.log"),
    );

    restore_codex_home(prev_codex_home);

    assert_eq!(
        transcripts.get("a"),
        Some(&first_rollout),
        "Codex pane A should not bind to the rollout already owned by pane B"
    );
    assert_eq!(
        transcripts.get("b"),
        Some(&second_rollout),
        "Codex pane B should keep its existing rollout binding"
    );
}

#[tokio::test]
async fn codex_transcript_binding_does_not_fallback_to_unmatched_rollout() {
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
        .join("28")
        .join("rollout-other-pane.jsonl");
    write_codex_rollout(
        &rollout,
        &cwd,
        session_ts,
        "PROMPT_FROM_OTHER_PANE",
        "OTHER_PANE_ANSWER",
    );

    let mut transcripts = HashMap::new();
    let pending_prompts = HashMap::from([(
        "a".to_string(),
        PendingPrompt::new("PROMPT_FROM_A".to_string()),
    )]);

    ensure_codex_transcript_local(
        "a",
        &mut transcripts,
        &pending_prompts,
        &cwd,
        started_at,
        &temp.path().join("relay.log"),
    );

    restore_codex_home(prev_codex_home);

    assert!(
        !transcripts.contains_key("a"),
        "Codex pane A should not bind an unrelated recent rollout"
    );
}

#[tokio::test]
async fn codex_transcript_binding_rejects_prompt_from_before_pending_prompt() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let prompt_recorded_at = chrono::Utc::now();
    let stale_prompt_at = prompt_recorded_at - chrono::Duration::minutes(5);
    let fresh_prompt_at = prompt_recorded_at + chrono::Duration::seconds(1);
    let prompt = "SAME_RELAY_PROMPT_FROM_OLDER_PAIR";
    let stale_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("06")
        .join("18")
        .join("rollout-stale.jsonl");
    let fresh_rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("06")
        .join("19")
        .join("rollout-fresh.jsonl");
    write_codex_rollout_with_message_timestamp(
        &stale_rollout,
        &cwd,
        stale_prompt_at,
        stale_prompt_at,
        prompt,
        "STALE_ANSWER",
    );
    write_codex_rollout_with_message_timestamp(
        &fresh_rollout,
        &cwd,
        fresh_prompt_at,
        fresh_prompt_at,
        prompt,
        "FRESH_ANSWER",
    );

    let mut transcripts = HashMap::new();
    let pending_prompts = HashMap::from([(
        "a".to_string(),
        PendingPrompt {
            text: prompt.to_string(),
            recorded_at: prompt_recorded_at,
        },
    )]);

    ensure_codex_transcript_local(
        "a",
        &mut transcripts,
        &pending_prompts,
        &cwd,
        started_at,
        &temp.path().join("relay.log"),
    );

    restore_codex_home(prev_codex_home);

    assert_eq!(
        transcripts.get("a"),
        Some(&fresh_rollout),
        "Codex pane should ignore stale same-prompt rollouts from older pairs"
    );
}

#[tokio::test]
async fn codex_manual_input_keeps_existing_transcript_binding() {
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
        .join("rollout-manual.jsonl");
    let first_prompt = "FIRST_PROMPT";
    let first_answer = "FIRST_CODEX_TO_A";
    write_codex_rollout(&rollout, &cwd, session_ts, first_prompt, first_answer);

    let pane_agents = HashMap::from([
        ("a".to_string(), "claude".to_string()),
        ("b".to_string(), "codex".to_string()),
    ]);

    let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
    let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
    let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let inputs = RelayInputs {
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
    };

    let handle = tokio::spawn(run(inputs));

    input_tx
        .send(("b".to_string(), first_prompt.to_string()))
        .await
        .unwrap();
    let first_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
    assert!(
        first_writes
            .iter()
            .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(first_answer)),
        "expected first codex answer to relay"
    );

    let second_prompt = "MANUAL_INTERVENTION_PROMPT";
    let second_answer = "SECOND_CODEX_TO_A";
    write_codex_rollout(&rollout, &cwd, session_ts, second_prompt, second_answer);
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
            "expected manual Codex input to keep the existing rollout binding and relay the next answer"
        );
}
