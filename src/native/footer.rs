use std::time::Duration;

pub(crate) fn mode_glyph(mode: &str) -> &'static str {
    match mode {
        "ON" => "▶",
        "PAUSE" => "⏸",
        "STOP" => "⏹",
        _ => "·",
    }
}

pub(crate) fn focus_caret(is_focused: bool) -> &'static str {
    if is_focused {
        "◀"
    } else {
        " "
    }
}

pub(crate) fn build_channel_dot() -> &'static str {
    let v = env!("CARGO_PKG_VERSION");
    if v.contains("-dev") || v.contains("-rc") {
        "●dev"
    } else if v.contains("-beta") {
        "●beta"
    } else {
        "●"
    }
}

pub(crate) fn queue_gauge_glyph(n: usize) -> &'static str {
    match n {
        0 => "",
        1 => " ▁",
        2 => " ▂",
        3..=4 => " ▃",
        5..=8 => " ▄",
        9..=16 => " ▅",
        17..=32 => " ▆",
        33..=64 => " ▇",
        _ => " █",
    }
}

pub(crate) fn uptime_label(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub(crate) fn pingpong_dot(elapsed: Duration) -> &'static str {
    match (elapsed.as_millis() / 500) % 3 {
        0 => "..>",
        1 => ".>.",
        _ => ">..",
    }
}

pub(crate) fn broadcast_caret_glyph(elapsed: Duration) -> &'static str {
    match (elapsed.as_millis() / 350) % 3 {
        0 => "▏",
        1 => "▎",
        _ => "▍",
    }
}

pub(crate) fn stop_warn_glyph(elapsed: Duration) -> &'static str {
    if (elapsed.as_millis() / 250) % 2 == 0 {
        "!"
    } else {
        " "
    }
}

pub(crate) fn traffic_sparkline(samples: &[u64]) -> String {
    let max = samples.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return "▁".repeat(samples.len().min(8));
    }
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    samples
        .iter()
        .take(8)
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * 7.0).round() as usize;
            bars[idx.min(7)]
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
}

pub(crate) fn direction_arrow(
    direction: Direction,
    active: bool,
    recently_hit: bool,
) -> &'static str {
    match (active, recently_hit) {
        (false, _) => "─x─",
        (true, true) => match direction {
            Direction::Left => "━◀━",
            Direction::Right => "━▶━",
        },
        (true, false) => match direction {
            Direction::Left => "─◀─",
            Direction::Right => "─▶─",
        },
    }
}

pub(crate) fn activity_dot(bytes_last_sec: u64) -> &'static str {
    match bytes_last_sec {
        0 => "·",
        1..=128 => "∘",
        _ => "●",
    }
}

pub(crate) fn hook_ping_glyph(since_last: Option<Duration>) -> &'static str {
    match since_last {
        Some(d) if d < Duration::from_millis(400) => "·",
        Some(d) if d < Duration::from_secs(10) => " ",
        Some(_) => "?", // long silence after we've seen at least one ping
        None => " ",    // never received → neutral
    }
}

pub(crate) fn marquee_window(line: &str, width: usize, offset: usize) -> String {
    if line.chars().count() <= width || width == 0 {
        return line.to_string();
    }
    let padded: String = format!("{line}     ");
    let total = padded.chars().count();
    let start = offset % total;
    padded.chars().cycle().skip(start).take(width).collect()
}

pub(crate) fn error_toast_fade(msg: &str, elapsed: Duration) -> Option<String> {
    let ms = elapsed.as_millis();
    if ms >= 4_000 {
        return None;
    }
    let glyph = match ms {
        0..=999 => '█',
        1_000..=1_999 => '▓',
        2_000..=2_999 => '▒',
        _ => '░',
    };
    Some(format!("{glyph} {msg}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_glyph_known_modes() {
        assert_eq!(mode_glyph("ON"), "▶");
        assert_eq!(mode_glyph("PAUSE"), "⏸");
        assert_eq!(mode_glyph("STOP"), "⏹");
        assert_eq!(mode_glyph("???"), "·");
    }

    #[test]
    fn focus_caret_toggles() {
        assert_eq!(focus_caret(true), "◀");
        assert_eq!(focus_caret(false), " ");
    }

    #[test]
    fn queue_gauge_scale() {
        assert_eq!(queue_gauge_glyph(0), "");
        assert_eq!(queue_gauge_glyph(1), " ▁");
        assert_eq!(queue_gauge_glyph(8), " ▄");
        assert_eq!(queue_gauge_glyph(65), " █");
        assert_eq!(queue_gauge_glyph(10_000), " █");
    }

    #[test]
    fn build_channel_dot_starts_with_marker() {
        assert!(build_channel_dot().starts_with('●'));
    }

    #[test]
    fn uptime_label_formats() {
        assert_eq!(uptime_label(Duration::from_secs(0)), "00:00");
        assert_eq!(uptime_label(Duration::from_secs(75)), "01:15");
        assert_eq!(uptime_label(Duration::from_secs(3725)), "1h02m");
    }

    #[test]
    fn pingpong_cycles_with_fixed_width_frames() {
        let frames: Vec<&str> = (0..4)
            .map(|i| pingpong_dot(Duration::from_millis(i * 500)))
            .collect();
        assert_eq!(frames, vec!["..>", ".>.", ">..", "..>"]);
        assert!(frames.iter().all(|frame| frame.chars().count() == 3));
    }

    #[test]
    fn stop_warn_blinks() {
        assert_eq!(stop_warn_glyph(Duration::from_millis(0)), "!");
        assert_eq!(stop_warn_glyph(Duration::from_millis(250)), " ");
    }

    #[test]
    fn broadcast_caret_widens_or_narrows() {
        let a = broadcast_caret_glyph(Duration::from_millis(0));
        let b = broadcast_caret_glyph(Duration::from_millis(350));
        let c = broadcast_caret_glyph(Duration::from_millis(700));
        assert!(a != b && b != c);
    }

    #[test]
    fn sparkline_empty_is_baseline() {
        assert_eq!(traffic_sparkline(&[0, 0, 0]), "▁▁▁");
    }

    #[test]
    fn sparkline_scales_to_max() {
        let s = traffic_sparkline(&[0, 50, 100]);
        assert_eq!(s.chars().count(), 3);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars[0], '▁');
        assert_eq!(chars[2], '█');
    }

    #[test]
    fn direction_arrow_states() {
        assert_eq!(direction_arrow(Direction::Right, false, false), "─x─");
        assert_eq!(direction_arrow(Direction::Right, true, false), "─▶─");
        assert_eq!(direction_arrow(Direction::Right, true, true), "━▶━");
        assert_eq!(direction_arrow(Direction::Left, true, false), "─◀─");
        assert_eq!(direction_arrow(Direction::Left, true, true), "━◀━");
    }

    #[test]
    fn activity_dot_thresholds() {
        assert_eq!(activity_dot(0), "·");
        assert_eq!(activity_dot(50), "∘");
        assert_eq!(activity_dot(10_000), "●");
    }

    #[test]
    fn hook_ping_glyph_phases() {
        assert_eq!(hook_ping_glyph(None), " ");
        assert_eq!(hook_ping_glyph(Some(Duration::from_millis(100))), "·");
        assert_eq!(hook_ping_glyph(Some(Duration::from_secs(3))), " ");
        assert_eq!(hook_ping_glyph(Some(Duration::from_secs(60))), "?");
    }

    #[test]
    fn marquee_window_scrolls() {
        let line = "abcdef";
        assert_eq!(marquee_window(line, 10, 0), "abcdef");
        let w = marquee_window(line, 4, 0);
        assert_eq!(w.chars().count(), 4);
        let w2 = marquee_window(line, 4, 1);
        assert_ne!(w, w2);
    }

    #[test]
    fn error_toast_fades_then_expires() {
        let m = "boom";
        assert!(error_toast_fade(m, Duration::from_millis(0))
            .unwrap()
            .starts_with('█'));
        assert!(error_toast_fade(m, Duration::from_millis(1500))
            .unwrap()
            .starts_with('▓'));
        assert!(error_toast_fade(m, Duration::from_millis(3500))
            .unwrap()
            .starts_with('░'));
        assert!(error_toast_fade(m, Duration::from_millis(4500)).is_none());
    }
}
