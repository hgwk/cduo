use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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
        Paragraph::new(footer_msg).style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
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
