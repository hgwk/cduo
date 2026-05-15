pub(crate) fn mode_glyph(mode: &str) -> &'static str {
    match mode {
        "ON" => "▶",
        "PAUSE" => "⏸",
        "STOP" => "⏹",
        _ => "·",
    }
}

pub(crate) fn focus_caret(is_focused: bool) -> &'static str {
    if is_focused { "◀" } else { " " }
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
}
