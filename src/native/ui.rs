use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use vt100::Color as VtColor;

/// Renders a vt100 screen into a ratatui buffer cell-by-cell, preserving
/// foreground/background colors and basic attributes (bold, italic, underline,
/// reverse). Wide characters take two columns; their continuation cells are
/// skipped so they don't overwrite the trailing half.
pub struct ScreenWidget<'a> {
    pub screen: &'a vt100::Screen,
    pub selection: Option<SelectionRange>,
}

impl<'a> Widget for ScreenWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (rows, cols) = self.screen.size();
        for row in 0..rows {
            if row >= area.height {
                break;
            }
            let mut col: u16 = 0;
            while col < cols && col < area.width {
                let Some(cell) = self.screen.cell(row, col) else {
                    col += 1;
                    continue;
                };
                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }
                let contents = cell.contents();
                let symbol: &str = if contents.is_empty() { " " } else { &contents };
                let mut style = vt_cell_style(cell);
                if self
                    .selection
                    .is_some_and(|selection| selection.contains(row, col))
                {
                    style = style.bg(Color::Cyan).fg(Color::Black);
                }
                let x = area.x + col;
                let y = area.y + row;
                if let Some(target) = buf.cell_mut(Position::new(x, y)) {
                    target.set_symbol(symbol).set_style(style);
                }
                col += if cell.is_wide() { 2 } else { 1 };
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

impl SelectionRange {
    pub fn normalized(self) -> Self {
        if (self.start_row, self.start_col) <= (self.end_row, self.end_col) {
            self
        } else {
            Self {
                start_row: self.end_row,
                start_col: self.end_col,
                end_row: self.start_row,
                end_col: self.start_col,
            }
        }
    }

    pub fn contains(self, row: u16, col: u16) -> bool {
        let range = self.normalized();
        if row < range.start_row || row > range.end_row {
            return false;
        }
        if range.start_row == range.end_row {
            return col >= range.start_col && col <= range.end_col;
        }
        if row == range.start_row {
            return col >= range.start_col;
        }
        if row == range.end_row {
            return col <= range.end_col;
        }
        true
    }
}

fn vt_cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default()
        .fg(vt_color_to_rat(cell.fgcolor()))
        .bg(vt_color_to_rat(cell.bgcolor()));
    let mut mods = Modifier::empty();
    if cell.bold() {
        mods |= Modifier::BOLD;
    }
    if cell.italic() {
        mods |= Modifier::ITALIC;
    }
    if cell.underline() {
        mods |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        mods |= Modifier::REVERSED;
    }
    style = style.add_modifier(mods);
    style
}

fn vt_color_to_rat(color: VtColor) -> Color {
    match color {
        VtColor::Default => Color::Reset,
        VtColor::Idx(i) => Color::Indexed(i),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Compute the per-pane PTY size given the full terminal frame size and a
/// 2-pane horizontal split. We reserve 1 row for header + 1 row for footer,
/// and split the remaining width in half (with a 1-column divider).
pub fn pane_pty_size(frame_cols: u16, frame_rows: u16) -> (u16, u16) {
    let inner_rows = frame_rows.saturating_sub(2).max(5);
    // 1-column divider between panes; the right pane absorbs any odd column.
    let usable_cols = frame_cols.saturating_sub(1);
    let pane_cols = (usable_cols / 2).max(20);
    (pane_cols, inner_rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_pty_size_basic() {
        let (cols, rows) = pane_pty_size(200, 50);
        assert_eq!(cols, 99);
        assert_eq!(rows, 48);
    }

    #[test]
    fn pane_pty_size_minimum_clamps() {
        let (cols, rows) = pane_pty_size(10, 4);
        assert!(cols >= 20);
        assert!(rows >= 5);
    }
}
