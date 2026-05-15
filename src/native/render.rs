use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::cli::SplitLayout;
use crate::native::layout::pane_layouts_for_view;
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::selection::MouseSelection;
use crate::native::ui::{ScreenWidget, SelectionRange};

pub(crate) fn draw(
    frame: &mut ratatui::Frame,
    panes: &[Pane; 2],
    focus: Focus,
    footer_msg: &str,
    selection: Option<MouseSelection>,
    split: SplitLayout,
    maximized: Option<PaneId>,
) {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        frame.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let (layouts, divider_area) = pane_layouts_for_view(area, split, maximized);

    frame.render_widget(
        Paragraph::new(format!(
            " cduo · A:{} | B:{} ",
            panes[0].agent, panes[1].agent
        ))
        .style(Style::default().add_modifier(Modifier::BOLD)),
        header_area,
    );

    if layouts[0].outer.width > 0 && layouts[0].outer.height > 0 {
        render_pane(
            frame,
            &panes[0],
            layouts[0].outer,
            focus.0 == PaneId::A,
            selection
                .filter(|selection| selection.pane == PaneId::A)
                .map(MouseSelection::range),
        );
    }
    if divider_area.width > 0 && divider_area.height > 0 {
        render_divider(frame, divider_area, split);
    }
    if layouts[1].outer.width > 0 && layouts[1].outer.height > 0 {
        render_pane(
            frame,
            &panes[1],
            layouts[1].outer,
            focus.0 == PaneId::B,
            selection
                .filter(|selection| selection.pane == PaneId::B)
                .map(MouseSelection::range),
        );
    }

    frame.render_widget(
        Paragraph::new(styled_footer_line(footer_msg, footer_area.width))
            .style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
}

fn styled_footer_line(message: &str, width: u16) -> Line<'static> {
    let message = footer_with_version(message, width);
    Line::from(
        message
            .split_inclusive(' ')
            .map(|part| {
                let token = part.trim_end();
                match footer_token_style(token) {
                    Some(style) => Span::styled(part.to_string(), style),
                    None => Span::raw(part.to_string()),
                }
            })
            .collect::<Vec<_>>(),
    )
}

fn footer_with_version(message: &str, width: u16) -> String {
    let version = footer_version_label();
    let message = message.trim_end();
    let message_len = message.chars().count();
    let version_len = version.chars().count();
    let width = usize::from(width);

    if width > message_len + version_len {
        let padding = width - message_len - version_len;
        format!("{message}{}{version}", " ".repeat(padding))
    } else {
        format!("{message} {version}")
    }
}

fn footer_version_label() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

fn footer_token_style(token: &str) -> Option<Style> {
    if token == "broadcast>" {
        return Some(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    }

    if token.starts_with("Ctrl-") || matches!(token, "Enter:" | "Esc:" | "PageUp/PageDown:") {
        return Some(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    }

    if token.strip_prefix("hook:").is_some() {
        return Some(Style::default().fg(Color::Blue));
    }

    if token
        .strip_prefix("q[")
        .is_some_and(|value| value.ends_with(']'))
    {
        return Some(Style::default().fg(Color::Cyan));
    }

    if matches!(token, "●" | "○") {
        return Some(Style::default().fg(Color::Yellow));
    }

    if token.chars().count() == 1
        && matches!(
            token.chars().next(),
            Some('▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')
        )
    {
        return Some(Style::default().fg(Color::Cyan));
    }

    let state = footer_indicator_state(token)?;

    let color = match state {
        "ON" => Color::Green,
        "OFF" | "STOP" => Color::Red,
        "PAUSE" => Color::Yellow,
        _ => return None,
    };

    Some(Style::default().fg(color).add_modifier(Modifier::BOLD))
}

fn footer_indicator_state(token: &str) -> Option<&str> {
    token
        .strip_prefix("relay[")
        .or_else(|| token.strip_prefix("A=>B["))
        .or_else(|| token.strip_prefix("B=>A["))
        .and_then(|value| value.strip_suffix(']'))
}

fn render_pane(
    frame: &mut ratatui::Frame,
    pane: &Pane,
    area: Rect,
    focused: bool,
    selection: Option<SelectionRange>,
) {
    let scroll = pane.scrollback();
    let title = if scroll > 0 {
        format!(
            " {} {} ↑{} ",
            pane.id.label().to_uppercase(),
            pane.agent,
            scroll
        )
    } else {
        format!(" {} {} ", pane.id.label().to_uppercase(), pane.agent)
    };
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let widget = ScreenWidget {
        screen: pane.parser.screen(),
        selection,
    };
    frame.render_widget(widget, inner);

    if focused && !pane.parser.screen().hide_cursor() {
        let (cur_row, cur_col) = pane.parser.screen().cursor_position();
        let x = inner.x + cur_col;
        let y = inner.y + cur_row;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            frame.set_cursor_position(ratatui::layout::Position::new(x, y));
        }
    }
}

fn render_divider(frame: &mut ratatui::Frame, area: Rect, split: SplitLayout) {
    let borders = match split {
        SplitLayout::Columns => Borders::LEFT,
        SplitLayout::Rows => Borders::TOP,
    };
    let block = Block::default()
        .borders(borders)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block, area);
}

#[cfg(test)]
mod tests {
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
        let footer = footer_with_version(" relay:on │ ready ", 24);

        assert_eq!(
            footer,
            format!(" relay:on │ ready {}", footer_version_label())
        );
        assert_eq!(footer.chars().count(), 24);
    }

    #[test]
    fn footer_version_is_appended_when_width_is_tight() {
        let footer = footer_with_version(" relay:on │ ready ", 10);

        assert_eq!(
            footer,
            format!(" relay:on │ ready {}", footer_version_label())
        );
    }
}
