    #[tokio::test]
    async fn relay_publishes_codex_polling_to_a() {
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
            .join("rollout-test.jsonl");
        let prompt = "RELAY_TEST_PROMPT";
        let answer = "RELAY_TEST_CODEX_TO_A";
        write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
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
        };

        let handle = tokio::spawn(run(inputs));

        // Pretend the user typed the prompt into pane B; this primes the
        // pending-prompt match so the relay can bind the rollout file.
        input_tx
            .send(("b".to_string(), prompt.to_string()))
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            !writes.is_empty(),
            "expected codex relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(
                target, "a",
                "Codex pane B should relay only to pane A, got target {target}"
            );
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
    async fn communication_gate_codex_to_claude() {
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
            .join("rollout-codex-a.jsonl");
        let prompt = "RELAY_TEST_PROMPT_FROM_A";
        let answer = "RELAY_TEST_CODEX_A_TO_CLAUDE_B";
        write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
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
        };

        let handle = tokio::spawn(run(inputs));

        input_tx
            .send(("a".to_string(), prompt.to_string()))
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            !writes.is_empty(),
            "expected codex relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(
                target, "b",
                "Codex pane A should relay only to pane B, got target {target}"
            );
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
