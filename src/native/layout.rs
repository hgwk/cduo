use ratatui::layout::Rect;

use crate::cli::SplitLayout;
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::ui::pane_pty_size;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneLayout {
    pub(crate) outer: Rect,
}

pub(crate) fn focus_index(focus: Focus) -> usize {
    pane_id_index(focus.0)
}

pub(crate) fn pane_id_index(id: PaneId) -> usize {
    match id {
        PaneId::A => 0,
        PaneId::B => 1,
    }
}

pub(crate) fn resize_panes(panes: &mut [Pane; 2], cols: u16, rows: u16, split: SplitLayout) {
    let (pane_cols, pane_rows) = pane_pty_size(cols, rows, split);
    for pane in panes.iter_mut() {
        pane.resize(pane_cols, pane_rows);
    }
}

pub(crate) fn toggle_split(split: SplitLayout) -> SplitLayout {
    match split {
        SplitLayout::Columns => SplitLayout::Rows,
        SplitLayout::Rows => SplitLayout::Columns,
    }
}

pub(crate) fn split_label(split: SplitLayout) -> &'static str {
    match split {
        SplitLayout::Columns => "columns",
        SplitLayout::Rows => "rows",
    }
}

pub(crate) fn pane_layouts(area: Rect, split: SplitLayout) -> ([PaneLayout; 2], Rect) {
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    match split {
        SplitLayout::Columns => {
            let half = body.width / 2;
            let pane_a = Rect::new(body.x, body.y, half, body.height);
            let divider = Rect::new(body.x + half, body.y, 1, body.height);
            let pane_b = Rect::new(
                body.x + half + 1,
                body.y,
                body.width.saturating_sub(half + 1),
                body.height,
            );
            (
                [PaneLayout { outer: pane_a }, PaneLayout { outer: pane_b }],
                divider,
            )
        }
        SplitLayout::Rows => {
            let half = body.height / 2;
            let pane_a = Rect::new(body.x, body.y, body.width, half);
            let divider = Rect::new(body.x, body.y + half, body.width, 1);
            let pane_b = Rect::new(
                body.x,
                body.y + half + 1,
                body.width,
                body.height.saturating_sub(half + 1),
            );
            (
                [PaneLayout { outer: pane_a }, PaneLayout { outer: pane_b }],
                divider,
            )
        }
    }
}

pub(crate) fn pane_inner(area: Rect) -> Rect {
    ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(area)
}

pub(crate) fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_split_switches_layouts() {
        assert_eq!(toggle_split(SplitLayout::Columns), SplitLayout::Rows);
        assert_eq!(toggle_split(SplitLayout::Rows), SplitLayout::Columns);
        assert_eq!(split_label(SplitLayout::Columns), "columns");
        assert_eq!(split_label(SplitLayout::Rows), "rows");
    }
}
