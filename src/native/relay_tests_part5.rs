    #[tokio::test]
    async fn communication_gate_claude_to_claude() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude.jsonl");
        let answer = "RELAY_TEST_CLAUDE_TO_B";
        write_claude_transcript(&transcript_path, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
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
        };

        let handle = tokio::spawn(run(inputs));

        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert!(
            !writes.is_empty(),
            "expected relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(target, "b", "Claude pane A should relay only to pane B");
        }
        let body = writes
            .iter()
            .find_map(|(_, bytes)| {
                let s = String::from_utf8_lossy(bytes);
                s.contains("\x1b[200~").then_some(bytes.clone())
            })
            .expect("expected at least one bracketed-paste bundle");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(answer), "paste body missing answer: {body:?}");
        assert!(
            writes.iter().any(|(_, b)| b == b"\r"),
            "expected trailing Enter byte"
        );
    }

    #[tokio::test]
    async fn communication_gate_claude_to_codex() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude-to-codex.jsonl");
        let answer = "COMM_GATE_CLAUDE_TO_CODEX";
        write_claude_transcript(&transcript_path, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
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

        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert_relay_writes(&writes, "b", answer);
    }

    #[tokio::test]
    async fn communication_gate_claude_to_codex_without_hook_transcript_path() {
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
            .join("claude-fallback.jsonl");
        let answer = "COMM_GATE_CLAUDE_TO_CODEX_FALLBACK";
        write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
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
                terminal_id: "a".to_string(),
                transcript_path: None,
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        restore_claude_home(prev_claude_home);

        assert_relay_writes(&writes, "b", answer);
    }
