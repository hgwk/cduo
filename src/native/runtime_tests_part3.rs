    #[test]
    fn footer_routes_use_pingpong_without_direction_arrows_when_both_routes_enabled() {
        let traffic = test_traffic();
        let footer = footer_with_relay_status(RelayStatusView {
            message: "ready",
            relay_paused: false,
            queued_writes: 0,
            a_to_b_enabled: true,
            b_to_a_enabled: true,
            relay_auto_stopped: false,
            heartbeat: false,
            elapsed: Duration::from_secs(0),
            traffic: &traffic,
            now: Instant::now(),
        });

        assert!(footer.contains("..●"));
        assert!(!footer.contains("─◀─"));
        assert!(!footer.contains("─▶─"));
    }

    #[test]
    fn footer_status_shows_stopped_relay_over_pause_state() {
        let traffic = test_traffic();
        let footer = footer_with_relay_status(RelayStatusView {
            message: "ready",
            relay_paused: true,
            queued_writes: 3,
            a_to_b_enabled: true,
            b_to_a_enabled: true,
            relay_auto_stopped: true,
            heartbeat: true,
            elapsed: Duration::from_secs(0),
            traffic: &traffic,
            now: Instant::now(),
        });

        assert!(footer.contains("⏹"));
        assert!(footer.contains("relay[STOP]"));
        assert!(!footer.contains("relay[PAUSE]"));
        assert!(!footer.contains('○'));
        assert!(footer.contains("up 00:00"));
    }

    #[test]
    fn route_footer_uses_ascii_indicator() {
        let footer = route_footer("A→B", false);

        assert!(footer.contains("route[A=>B:OFF]"));
        assert!(!footer.contains("A→B"));
    }
