use std::io;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::cli::SplitLayout;
use crate::native::layout::{pane_id_index, pane_layouts_for_view, PaneLayout};
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::runtime_io::{handle_mouse_wheel, SCROLL_LINES};
use crate::native::selection::{
    copy_to_clipboard_osc52, mouse_cell, mouse_cell_in_pane_clamped, mouse_pane, selected_text,
    MouseSelection,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_mouse_event(
    mouse: MouseEvent,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    panes: &mut [Pane; 2],
    focus: &mut Focus,
    split: SplitLayout,
    maximized: Option<PaneId>,
    selection: &mut Option<MouseSelection>,
    default_footer_msg: &str,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
    dirty: &mut bool,
) -> Result<()> {
    let size = terminal.size()?;
    let area = Rect::new(0, 0, size.width, size.height);
    let (layouts, _) = pane_layouts_for_view(area, split, maximized);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((pane, row, col)) = mouse_cell(mouse.column, mouse.row, layouts) {
                *focus = Focus(pane);
                *selection = Some(MouseSelection {
                    pane,
                    start_row: row,
                    start_col: col,
                    end_row: row,
                    end_col: col,
                });
                *footer_msg = default_footer_msg.to_string();
                *error_set_at = None;
                *dirty = true;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(active) = selection.as_mut() {
                if let Some((_, row, col)) =
                    mouse_cell_in_pane_clamped(mouse.column, mouse.row, layouts, active.pane)
                {
                    active.end_row = row;
                    active.end_col = col;
                    *dirty = true;
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(active) = selection.take() {
                let pane_idx = pane_id_index(active.pane);
                let text = selected_text(panes[pane_idx].parser.screen(), active.range());
                if !text.is_empty() {
                    copy_to_clipboard_osc52(terminal, &text)?;
                    *footer_msg = format!(
                        " copied {} chars from pane {} ",
                        text.chars().count(),
                        active.pane.label().to_uppercase()
                    );
                    *error_set_at = None;
                }
                *dirty = true;
            }
        }
        MouseEventKind::ScrollUp => handle_mouse_scroll(
            mouse,
            MouseEventKind::ScrollUp,
            panes,
            focus.0,
            layouts,
            footer_msg,
            error_set_at,
            error_raw_msg,
            dirty,
        ),
        MouseEventKind::ScrollDown => handle_mouse_scroll(
            mouse,
            MouseEventKind::ScrollDown,
            panes,
            focus.0,
            layouts,
            footer_msg,
            error_set_at,
            error_raw_msg,
            dirty,
        ),
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse_scroll(
    mouse: MouseEvent,
    kind: MouseEventKind,
    panes: &mut [Pane; 2],
    fallback_focus: PaneId,
    layouts: [PaneLayout; 2],
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
    dirty: &mut bool,
) {
    if let Some((pane, row, col)) = mouse_cell(mouse.column, mouse.row, layouts) {
        handle_mouse_wheel(panes, pane, kind, row, col, footer_msg, error_set_at);
        if error_set_at.is_some() {
            *error_raw_msg = footer_msg.clone();
        }
    } else {
        let pane = mouse_pane(mouse.column, mouse.row, layouts).unwrap_or(fallback_focus);
        match kind {
            MouseEventKind::ScrollUp => panes[pane_id_index(pane)].scroll_up(SCROLL_LINES),
            MouseEventKind::ScrollDown => panes[pane_id_index(pane)].scroll_down(SCROLL_LINES),
            _ => {}
        }
    }
    *dirty = true;
}
