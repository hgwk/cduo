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

pub(crate) fn resize_panes_for_view(
    panes: &mut [Pane; 2],
    cols: u16,
    rows: u16,
    split: SplitLayout,
    maximized: Option<PaneId>,
) {
    if let Some(active) = maximized {
        let body_cols = cols.saturating_sub(2).max(1);
        let body_rows = rows.saturating_sub(4).max(1);
        for pane in panes.iter_mut() {
            if pane.id == active {
                pane.resize(body_cols, body_rows);
            } else {
                pane.resize(1, 1);
            }
        }
    } else {
        resize_panes(panes, cols, rows, split);
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

pub(crate) fn pane_layouts_for_view(
    area: Rect,
    split: SplitLayout,
    maximized: Option<PaneId>,
) -> ([PaneLayout; 2], Rect) {
    let Some(active) = maximized else {
        return pane_layouts(area, split);
    };
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    let hidden = Rect::new(body.x, body.y, 0, 0);
    let divider = Rect::new(body.x, body.y, 0, 0);
    match active {
        PaneId::A => (
            [PaneLayout { outer: body }, PaneLayout { outer: hidden }],
            divider,
        ),
        PaneId::B => (
            [PaneLayout { outer: hidden }, PaneLayout { outer: body }],
            divider,
        ),
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

    #[test]
    fn maximized_layout_hides_inactive_pane() {
        let area = Rect::new(0, 0, 100, 40);
        let (layouts, divider) = pane_layouts_for_view(area, SplitLayout::Columns, Some(PaneId::B));

        assert_eq!(layouts[0].outer.width, 0);
        assert_eq!(layouts[0].outer.height, 0);
        assert_eq!(layouts[1].outer, Rect::new(0, 1, 100, 38));
        assert_eq!(divider.width, 0);
        assert_eq!(divider.height, 0);
    }
}
