use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::cli::SplitLayout;
use crate::native::footer::{build_channel_dot, focus_caret};
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

    let session = panes[0]
        .session_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!("[{name}]"))
        .unwrap_or_default();
    let header_text = format!(
        " cduo{session} · A{}:{} | B{}:{} · {} ",
        focus_caret(focus.0 == PaneId::A),
        panes[0].display_label(),
        focus_caret(focus.0 == PaneId::B),
        panes[1].display_label(),
        build_channel_dot(),
    );
    frame.render_widget(
        Paragraph::new(header_text).style(Style::default().add_modifier(Modifier::BOLD)),
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
        render_divider(frame, divider_area, split, focus);
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
    let (message, right_status) = split_footer_right_status(message);
    let right = match right_status {
        Some(status) => format!("{status} {version}"),
        None => version,
    };
    let message_len = message.chars().count();
    let right_len = right.chars().count();
    let width = usize::from(width);

    if width > message_len + right_len {
        let padding = width - message_len - right_len;
        format!("{message}{}{right}", " ".repeat(padding))
    } else if message.is_empty() {
        right
    } else {
        format!("{message} {right}")
    }
}

fn split_footer_right_status(message: &str) -> (&str, Option<&str>) {
    match message.rsplit_once(" · ") {
        Some((left, status)) if status.starts_with("up ") => (left.trim_end(), Some(status)),
        _ => (message, None),
    }
}

fn footer_version_label() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

fn footer_token_style(token: &str) -> Option<Style> {
    if token == "!" {
        return Some(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    }

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

    if matches!(token, "◀" | "▶" | "⏸" | "⏹") {
        return Some(Style::default().fg(Color::Yellow));
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

    if token.ends_with("[OFF]") {
        return Some(Style::default().fg(Color::Red));
    }
    if token.ends_with("[HIT]") {
        return Some(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
    }
    if token.ends_with("[ON]") {
        return Some(Style::default().fg(Color::Green));
    }
    if matches!(token, "∘") {
        return Some(Style::default().fg(Color::DarkGray));
    }

    if matches!(token, "●" | "○") {
        return Some(Style::default().fg(Color::Yellow));
    }

    if matches!(token, "..●" | ".●." | "●..") {
        return Some(Style::default().fg(Color::Green));
    }

    if !token.is_empty()
        && token
            .chars()
            .all(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█'))
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

fn render_divider(frame: &mut ratatui::Frame, area: Rect, split: SplitLayout, focus: Focus) {
    let borders = match split {
        SplitLayout::Columns => Borders::LEFT,
        SplitLayout::Rows => Borders::TOP,
    };
    // Divider color shifts with focus: cyan when A is focused, yellow when B.
    // The header focus caret is the primary indicator; this is a secondary cue.
    let divider_color = match focus.0 {
        PaneId::A => Color::Cyan,
        PaneId::B => Color::Yellow,
    };
    let block = Block::default()
        .borders(borders)
        .border_style(Style::default().fg(divider_color));
    frame.render_widget(block, area);
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
