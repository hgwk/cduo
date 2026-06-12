use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::native::footer::{mode_glyph, pingpong_dot, queue_gauge_glyph, uptime_label};

const LOG_TICKER_STATUS_RESERVE: usize = 64;

pub(super) fn pause_footer(queued_writes: usize) -> String {
    format!(" relay paused · queued writes: {queued_writes} · Ctrl-P: resume ")
}

pub(super) fn relay_reset_footer() -> String {
    " relay restarted · auto relay ON ".to_string()
}

pub(super) fn clear_paused_writes(paused_writes: &mut VecDeque<(String, Vec<u8>)>) -> usize {
    let cleared = paused_writes.len();
    paused_writes.clear();
    cleared
}

pub(super) fn route_footer(route: &str, enabled: bool) -> String {
    let state = if enabled { "ON" } else { "OFF" };
    let route = match route {
        "A→B" => "A=>B",
        "B→A" => "B=>A",
        other => other,
    };
    format!(" route[{route}:{state}] · Ctrl-1: A=>B · Ctrl-2: B=>A ")
}

pub(super) struct TrafficCounters {
    pub(super) a_to_b_bytes: u64,
    pub(super) b_to_a_bytes: u64,
    pub(super) last_a_to_b_at: Option<Instant>,
    pub(super) last_b_to_a_at: Option<Instant>,
    pub(super) samples_a_to_b: VecDeque<u64>,
    pub(super) samples_b_to_a: VecDeque<u64>,
    pub(super) last_sample_at: Instant,
}

pub(super) fn record_relay_traffic(
    traffic: &mut TrafficCounters,
    target: &str,
    byte_len: u64,
    now: Instant,
) {
    match target {
        "b" => {
            traffic.a_to_b_bytes += byte_len;
            traffic.last_a_to_b_at = Some(now);
        }
        "a" => {
            traffic.b_to_a_bytes += byte_len;
            traffic.last_b_to_a_at = Some(now);
        }
        _ => {}
    }
}

pub(super) fn log_ticker_footer(line: &str, offset: usize, footer_width: u16) -> String {
    let chrome_width = " log:  ".chars().count();
    let display_width =
        usize::from(footer_width).saturating_sub(LOG_TICKER_STATUS_RESERVE + chrome_width);
    let window = if display_width == 0 {
        String::new()
    } else {
        crate::native::footer::marquee_window(line, display_width, offset)
    };
    format!(" log: {window} ")
}

pub(super) struct RelayStatusView<'a> {
    pub(super) message: &'a str,
    pub(super) relay_paused: bool,
    pub(super) queued_writes: usize,
    pub(super) a_to_b_enabled: bool,
    pub(super) b_to_a_enabled: bool,
    pub(super) relay_auto_stopped: bool,
    pub(super) heartbeat: bool,
    pub(super) elapsed: Duration,
    pub(super) traffic: &'a TrafficCounters,
    pub(super) now: Instant,
}

pub(super) fn footer_with_relay_status(view: RelayStatusView<'_>) -> String {
    use crate::native::footer::{
        activity_dot, route_status_token, stop_warn_glyph, traffic_sparkline,
    };
    let RelayStatusView {
        message,
        relay_paused,
        queued_writes,
        a_to_b_enabled,
        b_to_a_enabled,
        relay_auto_stopped,
        heartbeat,
        elapsed,
        traffic,
        now,
    } = view;

    let mode = if relay_auto_stopped {
        "STOP"
    } else if relay_paused {
        "PAUSE"
    } else {
        "ON"
    };
    let glyph = mode_glyph(mode);
    let warn = if relay_auto_stopped {
        stop_warn_glyph(elapsed)
    } else {
        ""
    };
    let pulse = if relay_paused && !relay_auto_stopped {
        if heartbeat {
            " ●"
        } else {
            " ○"
        }
    } else {
        ""
    };
    let gauge = queue_gauge_glyph(queued_writes);

    let pulse_a_to_b = traffic
        .last_a_to_b_at
        .map(|t| now.duration_since(t) < Duration::from_millis(200))
        .unwrap_or(false);
    let pulse_b_to_a = traffic
        .last_b_to_a_at
        .map(|t| now.duration_since(t) < Duration::from_millis(200))
        .unwrap_or(false);
    let spark_ab_vec: Vec<u64> = traffic.samples_a_to_b.iter().copied().collect();
    let spark_ba_vec: Vec<u64> = traffic.samples_b_to_a.iter().copied().collect();
    let spark_ab = traffic_sparkline(&spark_ab_vec);
    let spark_ba = traffic_sparkline(&spark_ba_vec);
    let act_a = activity_dot(*traffic.samples_b_to_a.back().unwrap_or(&0));
    let act_b = activity_dot(*traffic.samples_a_to_b.back().unwrap_or(&0));

    let routes = if !relay_paused && !relay_auto_stopped && a_to_b_enabled && b_to_a_enabled {
        let pp = pingpong_dot(elapsed);
        format!("A{act_a} {spark_ba} {pp} {spark_ab} {act_b}B")
    } else {
        let route_ab = route_status_token("ab", a_to_b_enabled, pulse_a_to_b);
        let route_ba = route_status_token("ba", b_to_a_enabled, pulse_b_to_a);
        format!("A{act_a} {route_ba} | {route_ab} {act_b}B")
    };
    let uptime = uptime_label(elapsed);
    format!(
        " {glyph}{warn} relay[{mode}]{pulse} · q[{queued_writes}]{gauge} · {routes} · {} · up {uptime}",
        message.trim()
    )
}
