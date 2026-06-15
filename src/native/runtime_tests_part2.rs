use super::*;
#[test]
fn metadata_key_buffer_edits_and_submits() {
    let mut buffer = String::new();

    assert_eq!(
        handle_metadata_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &mut buffer
        ),
        MetadataInputAction::Editing
    );
    assert_eq!(
        handle_metadata_key(
            KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE),
            &mut buffer
        ),
        MetadataInputAction::Editing
    );
    assert_eq!(buffer, "s=");
    assert_eq!(
        handle_metadata_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut buffer
        ),
        MetadataInputAction::Editing
    );
    assert_eq!(buffer, "s");
    assert_eq!(
        handle_metadata_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut buffer
        ),
        MetadataInputAction::Submit("s".to_string())
    );
}

#[test]
fn metadata_key_ctrl_n_cancels_mode() {
    let mut buffer = "session=api".to_string();

    assert_eq!(
        handle_metadata_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &mut buffer
        ),
        MetadataInputAction::Cancel
    );
}

#[test]
fn metadata_footer_names_controls() {
    let footer = metadata_input_footer("session=api a=planner b=builder");

    assert!(footer.contains("metadata> session=api a=planner b=builder"));
    assert!(footer.contains("Enter"));
    assert!(footer.contains("Esc"));
}

#[test]
fn parse_metadata_update_accepts_session_and_roles() {
    let update = parse_metadata_update("session=api a=planner b=builder").unwrap();

    assert_eq!(update.session_name, Some(Some("api".to_string())));
    assert_eq!(update.role_a, Some(Some("planner".to_string())));
    assert_eq!(update.role_b, Some(Some("builder".to_string())));
}

#[test]
fn parse_metadata_update_accepts_quoted_values_with_spaces() {
    let update =
        parse_metadata_update(r#"session="api team" a="code reviewer" b="ship builder""#).unwrap();

    assert_eq!(update.session_name, Some(Some("api team".to_string())));
    assert_eq!(update.role_a, Some(Some("code reviewer".to_string())));
    assert_eq!(update.role_b, Some(Some("ship builder".to_string())));
}

#[test]
fn current_metadata_input_quotes_values_with_spaces() {
    assert_eq!(
        metadata_input_value(
            Some("api team"),
            Some("code reviewer"),
            Some("ship builder")
        ),
        r#"session="api team" a="code reviewer" b="ship builder""#
    );
}

#[test]
fn parse_metadata_update_can_clear_fields() {
    let update = parse_metadata_update("session=- a=none b=").unwrap();

    assert_eq!(update.session_name, Some(None));
    assert_eq!(update.role_a, Some(None));
    assert_eq!(update.role_b, Some(None));
}

#[test]
fn parse_metadata_update_rejects_unknown_fields() {
    let err = parse_metadata_update("team=api").unwrap_err();

    assert!(err.contains("unknown key"));
}

#[test]
fn send_control_or_footer_reports_closed_control_channel() {
    let (tx, rx) = mpsc::channel::<relay::RelayControl>(1);
    drop(rx);

    let footer = send_control_or_footer(
        &tx,
        relay::RelayControl::SetPrefix(Some("prefix".to_string())),
        || "success".to_string(),
    );

    assert!(footer.contains("relay control unavailable"));
}

#[test]
fn write_error_footer_names_target_pane() {
    let footer = write_error_footer("a", &std::io::Error::other("closed"));
    assert!(footer.contains("pane a"));
    assert!(footer.contains("closed"));
}

#[test]
fn relay_reset_footer_names_auto_relay_on_state() {
    assert_eq!(relay_reset_footer(), " relay restarted · auto relay ON ");
}

#[test]
fn pane_env_includes_session_and_role_metadata() {
    let env = pane_env("a", "53333", Some("api"), Some("planner"));

    assert!(env.contains(&("TERMINAL_ID", "a")));
    assert!(env.contains(&("ORCHESTRATION_PORT", "53333")));
    assert!(env.contains(&("CDUO_SESSION_NAME", "api")));
    assert!(env.contains(&("CDUO_PANE_ROLE", "planner")));
}

#[test]
fn pane_env_skips_blank_metadata() {
    let env = pane_env("b", "53333", Some(" "), Some(""));

    assert!(env.contains(&("TERMINAL_ID", "b")));
    assert!(!env.iter().any(|(key, _)| *key == "CDUO_SESSION_NAME"));
    assert!(!env.iter().any(|(key, _)| *key == "CDUO_PANE_ROLE"));
}

fn test_traffic() -> TrafficCounters {
    TrafficCounters {
        a_to_b_bytes: 0,
        b_to_a_bytes: 0,
        last_a_to_b_at: None,
        last_b_to_a_at: None,
        samples_a_to_b: std::collections::VecDeque::from(vec![0u64; 8]),
        samples_b_to_a: std::collections::VecDeque::from(vec![0u64; 8]),
        last_sample_at: Instant::now(),
    }
}

#[test]
fn record_relay_traffic_counts_targets_and_marks_last_write() {
    let mut traffic = test_traffic();
    let now = Instant::now();

    record_relay_traffic(&mut traffic, "b", 42, now);
    record_relay_traffic(&mut traffic, "a", 7, now + Duration::from_millis(1));
    record_relay_traffic(&mut traffic, "unknown", 99, now + Duration::from_millis(2));

    assert_eq!(traffic.a_to_b_bytes, 42);
    assert_eq!(traffic.b_to_a_bytes, 7);
    assert_eq!(traffic.last_a_to_b_at, Some(now));
    assert_eq!(traffic.last_b_to_a_at, Some(now + Duration::from_millis(1)));
}

#[test]
fn log_ticker_footer_uses_footer_width_for_window() {
    let footer = log_ticker_footer("abcdefghijklmnopqrstuvwxyz", 0, 80);

    assert_eq!(footer, " log: abcdefghi ");
    assert_eq!(footer.chars().count(), 16);
}

#[test]
fn log_ticker_footer_scrolls_and_handles_tiny_width() {
    assert_eq!(log_ticker_footer("abcdefghijkl", 1, 75), " log: bcde ");
    assert_eq!(log_ticker_footer("abcdef", 0, 70), " log:  ");
}

#[test]
fn footer_status_shows_relay_mode_queue_and_routes() {
    let traffic = test_traffic();
    let footer = footer_with_relay_status(RelayStatusView {
        message: "ready",
        relay_paused: true,
        queued_writes: 3,
        a_to_b_enabled: false,
        b_to_a_enabled: true,
        relay_auto_stopped: false,
        heartbeat: true,
        elapsed: Duration::from_secs(0),
        traffic: &traffic,
        now: Instant::now(),
    });

    assert!(footer.contains("⏸"));
    assert!(footer.contains("relay[PAUSE] ●"));
    assert!(footer.contains("q[3] ▃"));
    assert!(footer.contains("ab[OFF]"));
    assert!(footer.contains("ba[ON]"));
    assert!(!footer.contains("─◀─"));
    assert!(!footer.contains("─▶─"));
    assert!(footer.contains("ready"));
    assert!(footer.contains("up 00:00"));
    assert!(footer.ends_with("up 00:00"));
}
