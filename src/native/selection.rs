use std::io;

use anyhow::{Context, Result};
use base64::Engine;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::native::layout::{pane_id_index, pane_inner, point_in_rect, PaneLayout};
use crate::native::pane::PaneId;
use crate::native::ui::SelectionRange;

#[derive(Debug, Clone, Copy)]
pub(crate) struct MouseSelection {
    pub(crate) pane: PaneId,
    pub(crate) start_row: u16,
    pub(crate) start_col: u16,
    pub(crate) end_row: u16,
    pub(crate) end_col: u16,
}

impl MouseSelection {
    pub(crate) fn range(self) -> SelectionRange {
        SelectionRange {
            start_row: self.start_row,
            start_col: self.start_col,
            end_row: self.end_row,
            end_col: self.end_col,
        }
    }
}

pub(crate) fn mouse_pane(col: u16, row: u16, layouts: [PaneLayout; 2]) -> Option<PaneId> {
    if point_in_rect(col, row, layouts[0].outer) {
        Some(PaneId::A)
    } else if point_in_rect(col, row, layouts[1].outer) {
        Some(PaneId::B)
    } else {
        None
    }
}

pub(crate) fn mouse_cell(
    col: u16,
    row: u16,
    layouts: [PaneLayout; 2],
) -> Option<(PaneId, u16, u16)> {
    mouse_cell_in_pane(col, row, layouts, PaneId::A)
        .or_else(|| mouse_cell_in_pane(col, row, layouts, PaneId::B))
}

pub(crate) fn mouse_cell_in_pane(
    col: u16,
    row: u16,
    layouts: [PaneLayout; 2],
    pane: PaneId,
) -> Option<(PaneId, u16, u16)> {
    let layout = layouts[pane_id_index(pane)];
    let inner = pane_inner(layout.outer);
    if !point_in_rect(col, row, inner) {
        let clamped_col = col.clamp(
            inner.x,
            inner.x.saturating_add(inner.width.saturating_sub(1)),
        );
        let clamped_row = row.clamp(
            inner.y,
            inner.y.saturating_add(inner.height.saturating_sub(1)),
        );
        if !point_in_rect(clamped_col, clamped_row, inner) {
            return None;
        }
        return Some((pane, clamped_row - inner.y, clamped_col - inner.x));
    }
    Some((pane, row - inner.y, col - inner.x))
}

pub(crate) fn selected_text(screen: &vt100::Screen, selection: SelectionRange) -> String {
    let range = selection.normalized();
    let mut lines = Vec::new();
    for row in range.start_row..=range.end_row {
        let start_col = if row == range.start_row {
            range.start_col
        } else {
            0
        };
        let end_col = if row == range.end_row {
            range.end_col
        } else {
            screen.size().1.saturating_sub(1)
        };
        let mut line = String::new();
        for col in start_col..=end_col {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let contents = cell.contents();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&contents);
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n").trim().to_string()
}

pub(crate) fn copy_to_clipboard_osc52(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    text: &str,
) -> Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let backend = terminal.backend_mut();
    use std::io::Write;
    write!(backend, "\x1b]52;c;{encoded}\x07").context("write OSC52 clipboard")?;
    backend.flush().context("flush OSC52 clipboard")?;
    Ok(())
}
