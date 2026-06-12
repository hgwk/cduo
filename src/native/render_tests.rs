use super::*;

#[test]
fn footer_status_tokens_are_colored_by_state() {
    let line = styled_footer_line(" relay[ON] q[0] A=>B[OFF] B=>A[ON] | ready ", 80);

    let relay = line
        .spans
        .iter()
        .find(|span| span.content == "relay[ON] ")
        .unwrap();
    let a_to_b = line
        .spans
        .iter()
        .find(|span| span.content == "A=>B[OFF] ")
        .unwrap();
    let b_to_a = line
        .spans
        .iter()
        .find(|span| span.content == "B=>A[ON] ")
        .unwrap();
    let queue = line
        .spans
        .iter()
        .find(|span| span.content == "q[0] ")
        .unwrap();

    assert_eq!(relay.style.fg, Some(Color::Green));
    assert_eq!(a_to_b.style.fg, Some(Color::Red));
    assert_eq!(b_to_a.style.fg, Some(Color::Green));
    assert_eq!(queue.style.fg, Some(Color::Cyan));
}

#[test]
fn footer_paused_and_stopped_use_warning_colors() {
    let paused = footer_token_style("relay[PAUSE]").unwrap();
    let stopped = footer_token_style("relay[STOP]").unwrap();

    assert_eq!(paused.fg, Some(Color::Yellow));
    assert_eq!(stopped.fg, Some(Color::Red));
}

#[test]
fn footer_tool_tokens_are_colored() {
    let line = styled_footer_line(
        " relay[ON] q[0] hook:53333 Ctrl-Y: broadcast Enter: send Esc: cancel ",
        120,
    );

    let hook = line
        .spans
        .iter()
        .find(|span| span.content == "hook:53333 ")
        .unwrap();
    let ctrl = line
        .spans
        .iter()
        .find(|span| span.content == "Ctrl-Y: ")
        .unwrap();
    let enter = line
        .spans
        .iter()
        .find(|span| span.content == "Enter: ")
        .unwrap();
    let queue = line
        .spans
        .iter()
        .find(|span| span.content == "q[0] ")
        .unwrap();

    assert_eq!(hook.style.fg, Some(Color::Blue));
    assert_eq!(ctrl.style.fg, Some(Color::Cyan));
    assert_eq!(enter.style.fg, Some(Color::Cyan));
    assert_eq!(queue.style.fg, Some(Color::Cyan));
}

#[test]
fn footer_broadcast_prompt_is_colored() {
    let line = styled_footer_line(" broadcast> compare outputs · Enter: send ", 80);
    let prompt = line
        .spans
        .iter()
        .find(|span| span.content == "broadcast> ")
        .unwrap();

    assert_eq!(prompt.style.fg, Some(Color::Magenta));
}

#[test]
fn footer_version_is_right_aligned_when_width_allows() {
    let expected = format!(" relay:on │ ready {}", footer_version_label());
    let width = expected.chars().count() as u16;
    let footer = footer_with_version(" relay:on │ ready ", width);

    assert_eq!(footer, expected);
    assert_eq!(footer.chars().count(), width as usize);
}

#[test]
fn footer_version_is_appended_when_width_is_tight() {
    let footer = footer_with_version(" relay:on │ ready ", 10);

    assert_eq!(
        footer,
        format!(" relay:on │ ready {}", footer_version_label())
    );
}

#[test]
fn footer_uptime_is_moved_next_to_right_version() {
    let footer = footer_with_version(" relay[ON] · ready · up 01:02 ", 48);
    let right = format!("up 01:02 {}", footer_version_label());

    assert!(footer.starts_with(" relay[ON] · ready"));
    assert!(footer.ends_with(&right));
    assert!(!footer.contains("· up 01:02"));
    assert_eq!(footer.chars().count(), 48);
}

#[test]
fn footer_uptime_and_version_append_when_tight() {
    let footer = footer_with_version(" relay[ON] · up 01:02 ", 10);

    assert_eq!(
        footer,
        format!(" relay[ON] up 01:02 {}", footer_version_label())
    );
}
