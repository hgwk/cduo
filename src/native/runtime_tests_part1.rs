    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn preferred_hook_port_defaults_when_env_missing_or_invalid() {
        let _guard = env_lock();
        std::env::remove_var("CDUO_PORT");
        std::env::set_var("PORT", "not-a-port");
        assert_eq!(preferred_hook_port(), 53333);
        std::env::remove_var("PORT");
    }

    #[test]
    fn preferred_hook_port_accepts_cduo_port_over_port() {
        let _guard = env_lock();
        std::env::set_var("PORT", "12345");
        std::env::set_var("CDUO_PORT", "23456");
        assert_eq!(preferred_hook_port(), 23456);
        std::env::remove_var("CDUO_PORT");
        std::env::remove_var("PORT");
    }

    #[test]
    fn candidate_hook_ports_stops_at_u16_max_without_overflow() {
        let ports: Vec<u16> = candidate_hook_ports(u16::MAX - 1).collect();
        assert_eq!(ports, vec![u16::MAX - 1, u16::MAX]);
    }

    #[tokio::test]
    async fn bind_hook_listener_skips_busy_port_and_keeps_listener_bound() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let busy_port = busy.local_addr().unwrap().port();

        let listener = bind_hook_listener(busy_port).await.unwrap();
        let selected_port = listener.local_addr().unwrap().port();

        assert_ne!(selected_port, busy_port);
        assert!(TcpListener::bind(("127.0.0.1", selected_port))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn capture_line_emits_on_cr() {
        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, b"hi", &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        capture_line(PaneId::A, b"\r", &mut buf, &tx);

        let (pane, text) = rx.try_recv().unwrap();
        assert_eq!(pane, "a");
        assert_eq!(text, "hi");
    }

    #[tokio::test]
    async fn capture_line_separates_panes() {
        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, b"alpha", &mut buf, &tx);
        capture_line(PaneId::B, b"beta\r", &mut buf, &tx);
        capture_line(PaneId::A, b"\r", &mut buf, &tx);

        let mut got: Vec<(String, String)> = Vec::new();
        while let Ok(item) = rx.try_recv() {
            got.push(item);
        }
        assert_eq!(
            got,
            vec![
                ("b".to_string(), "beta".to_string()),
                ("a".to_string(), "alpha".to_string()),
            ]
        );
    }

    #[test]
    fn clear_paused_writes_drops_all_queued_relay_bytes() {
        let mut queue = VecDeque::from([
            ("a".to_string(), b"one".to_vec()),
            ("b".to_string(), b"two".to_vec()),
        ]);

        assert_eq!(clear_paused_writes(&mut queue), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn mouse_wheel_bytes_use_sgr_coordinates() {
        assert_eq!(
            mouse_wheel_bytes(MouseEventKind::ScrollUp, 2, 3).unwrap(),
            b"\x1b[<64;4;3M"
        );
        assert_eq!(
            mouse_wheel_bytes(MouseEventKind::ScrollDown, 2, 3).unwrap(),
            b"\x1b[<65;4;3M"
        );
        assert!(mouse_wheel_bytes(MouseEventKind::Moved, 2, 3).is_none());
    }

    #[test]
    fn codex_screen_scroll_uses_page_key_sequences() {
        assert_eq!(
            codex_screen_scroll_bytes(&GlobalAction::ScrollUp).unwrap(),
            b"\x1b[5~"
        );
        assert_eq!(
            codex_screen_scroll_bytes(&GlobalAction::ScrollDown).unwrap(),
            b"\x1b[6~"
        );
        assert!(codex_screen_scroll_bytes(&GlobalAction::Forward).is_none());
    }

    #[test]
    fn broadcast_key_buffer_edits_and_submits() {
        let mut buffer = String::new();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "hi");

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "h");

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Submit("h".to_string())
        );
    }

    #[test]
    fn broadcast_key_ignores_control_chars_and_cancels() {
        let mut buffer = "keep".to_string();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "keep");
        assert_eq!(
            handle_broadcast_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut buffer),
            BroadcastInputAction::Cancel
        );
    }

    #[test]
    fn broadcast_key_ctrl_y_cancels_mode() {
        let mut buffer = "draft".to_string();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
                &mut buffer
            ),
            BroadcastInputAction::Cancel
        );
        assert_eq!(buffer, "draft");
    }

    #[test]
    fn broadcast_prompt_bytes_add_enter_and_capture_both_panes() {
        let bytes = broadcast_prompt_bytes("same prompt");
        assert_eq!(bytes, "User says: same prompt\r".as_bytes());

        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, &bytes, &mut buf, &tx);
        capture_line(PaneId::B, &bytes, &mut buf, &tx);

        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        assert_eq!(
            first,
            ("a".to_string(), "User says: same prompt".to_string())
        );
        assert_eq!(
            second,
            ("b".to_string(), "User says: same prompt".to_string())
        );
    }

    #[test]
    fn broadcast_footer_names_controls() {
        let footer = broadcast_input_footer("compare this", Duration::from_secs(0));

        assert!(footer.contains("broadcast> compare this"));
        assert!(footer.contains("Enter"));
        assert!(footer.contains("Esc"));
    }
