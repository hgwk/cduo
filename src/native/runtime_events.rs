use std::{
    collections::{HashMap, VecDeque},
    io,
    time::Instant,
};

use anyhow::Result;
use crossterm::event::{KeyEvent, KeyEventKind};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::cli::SplitLayout;
use crate::native::input::{classify_key, key_to_bytes, GlobalAction};
use crate::native::layout::{focus_index, resize_panes_for_view, split_label, toggle_split};
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::relay;
use crate::native::runtime_io::*;
use crate::native::runtime_loop_support::{
    recent_log_footer, send_control_or_footer, sync_maximized_focus,
};
use crate::native::runtime_metadata::*;
use crate::native::runtime_status::{
    clear_paused_writes, pause_footer, relay_reset_footer, route_footer,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_key_event(
    key: KeyEvent,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    panes: &mut [Pane; 2],
    focus: &mut Focus,
    split: &mut SplitLayout,
    maximized: &mut Option<PaneId>,
    broadcast_input: &mut Option<String>,
    metadata_input: &mut Option<String>,
    input_buf: &mut HashMap<PaneId, Vec<u8>>,
    input_tx: &mpsc::Sender<(String, String)>,
    control_tx: &mpsc::Sender<relay::RelayControl>,
    relay_paused: &mut bool,
    relay_auto_stopped: &mut bool,
    paused_writes: &mut VecDeque<(String, Vec<u8>)>,
    a_to_b_enabled: &mut bool,
    b_to_a_enabled: &mut bool,
    log_ticker_on: &mut bool,
    log_ticker_offset: &mut usize,
    log_ticker_last_tick: &mut Instant,
    log_ticker_line: &mut String,
    runtime_start: Instant,
    log_path: &std::path::Path,
    default_footer_msg: &str,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
    dirty: &mut bool,
) -> Result<bool> {
    if key.kind == KeyEventKind::Release {
        return Ok(false);
    }
    if (broadcast_input.is_some() || metadata_input.is_some())
        && classify_key(key) == GlobalAction::Quit
    {
        return Ok(true);
    }
    if let Some(buffer) = metadata_input.as_mut() {
        match handle_metadata_key(key, buffer) {
            MetadataInputAction::Editing => {
                *footer_msg = metadata_input_footer(buffer);
                *error_set_at = None;
            }
            MetadataInputAction::Cancel => {
                *metadata_input = None;
                *footer_msg = default_footer_msg.to_string();
                *error_set_at = None;
            }
            MetadataInputAction::Submit(input) => {
                *metadata_input = None;
                *footer_msg = match parse_metadata_update(&input) {
                    Ok(update) => apply_metadata_update(panes, update),
                    Err(err) => format!(" metadata unchanged · {err} "),
                };
                *error_set_at = None;
            }
        }
        *dirty = true;
        return Ok(false);
    }
    if let Some(buffer) = broadcast_input.as_mut() {
        match handle_broadcast_key(key, buffer) {
            BroadcastInputAction::Editing => {
                *footer_msg = broadcast_input_footer(buffer, runtime_start.elapsed());
                *error_set_at = None;
            }
            BroadcastInputAction::Cancel => {
                *broadcast_input = None;
                *footer_msg = default_footer_msg.to_string();
                *error_set_at = None;
            }
            BroadcastInputAction::Submit(prompt) => {
                *broadcast_input = None;
                send_broadcast_prompt(
                    panes,
                    &prompt,
                    input_buf,
                    input_tx,
                    footer_msg,
                    error_set_at,
                    error_raw_msg,
                );
                *relay_paused = true;
            }
        }
        *dirty = true;
        return Ok(false);
    }

    match classify_key(key) {
        GlobalAction::Quit => return Ok(true),
        GlobalAction::FocusNext => {
            *focus = focus.next();
            sync_maximized_focus(terminal, panes, *focus, *split, maximized)?;
            *dirty = true;
        }
        GlobalAction::FocusPrev => {
            *focus = focus.prev();
            sync_maximized_focus(terminal, panes, *focus, *split, maximized)?;
            *dirty = true;
        }
        GlobalAction::TogglePause => {
            *footer_msg = if *relay_auto_stopped {
                *relay_auto_stopped = false;
                *relay_paused = false;
                send_control_or_footer(
                    control_tx,
                    relay::RelayControl::ResetStop,
                    relay_reset_footer,
                )
            } else if *relay_paused {
                *relay_paused = false;
                default_footer_msg.to_string()
            } else {
                *relay_paused = true;
                pause_footer(paused_writes.len())
            };
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ToggleSplit => {
            *split = toggle_split(*split);
            let size = terminal.size()?;
            resize_panes_for_view(panes, size.width, size.height, *split, *maximized);
            *footer_msg = format!(" split: {} · Ctrl-L: toggle split ", split_label(*split));
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ManualRelay => {
            let pane_id = focus.0.label().to_string();
            *footer_msg = send_control_or_footer(
                control_tx,
                relay::RelayControl::ManualRelay {
                    pane_id: pane_id.clone(),
                },
                || {
                    format!(
                        " manual relay requested from pane {} ",
                        pane_id.to_uppercase()
                    )
                },
            );
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ClearRelayQueue => {
            let cleared = clear_paused_writes(paused_writes);
            *footer_msg = format!(" relay queue cleared · dropped writes: {cleared} ");
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ToggleRelayAToB => {
            *a_to_b_enabled = !*a_to_b_enabled;
            *footer_msg = send_control_or_footer(
                control_tx,
                relay::RelayControl::SetRoute {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    enabled: *a_to_b_enabled,
                },
                || route_footer("A→B", *a_to_b_enabled),
            );
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ToggleRelayBToA => {
            *b_to_a_enabled = !*b_to_a_enabled;
            *footer_msg = send_control_or_footer(
                control_tx,
                relay::RelayControl::SetRoute {
                    source: "b".to_string(),
                    target: "a".to_string(),
                    enabled: *b_to_a_enabled,
                },
                || route_footer("B→A", *b_to_a_enabled),
            );
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ShowRelayLog => {
            *footer_msg = recent_log_footer(log_path);
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ToggleFocusLayout => {
            *maximized = match *maximized {
                Some(active) if active == focus.0 => None,
                _ => Some(focus.0),
            };
            let size = terminal.size()?;
            resize_panes_for_view(panes, size.width, size.height, *split, *maximized);
            *footer_msg = match maximized {
                Some(active) => {
                    format!(
                        " pane {} maximized · Ctrl-Z: restore ",
                        active.label().to_uppercase()
                    )
                }
                None => " layout restored · Ctrl-Z: maximize focused pane ".to_string(),
            };
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::BroadcastInput => {
            *broadcast_input = Some(String::new());
            *footer_msg = broadcast_input_footer("", runtime_start.elapsed());
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::EditMetadata => {
            *metadata_input = Some(current_metadata_input(panes));
            *footer_msg = metadata_input_footer(metadata_input.as_deref().unwrap_or(""));
            *error_set_at = None;
            *dirty = true;
        }
        GlobalAction::ScrollUp => {
            handle_screen_scroll(
                panes,
                focus.0,
                &GlobalAction::ScrollUp,
                footer_msg,
                error_set_at,
                error_raw_msg,
            );
            *dirty = true;
        }
        GlobalAction::ScrollDown => {
            handle_screen_scroll(
                panes,
                focus.0,
                &GlobalAction::ScrollDown,
                footer_msg,
                error_set_at,
                error_raw_msg,
            );
            *dirty = true;
        }
        GlobalAction::ToggleLogTicker => {
            *log_ticker_on = !*log_ticker_on;
            *log_ticker_offset = 0;
            *log_ticker_last_tick = Instant::now();
            if *log_ticker_on {
                *log_ticker_line = std::fs::read_to_string(log_path)
                    .ok()
                    .and_then(|c| c.lines().last().map(str::to_string))
                    .unwrap_or_default();
            }
            *dirty = true;
            *error_set_at = None;
        }
        GlobalAction::Forward => {
            if let Some(bytes) = key_to_bytes(key) {
                let idx = focus_index(*focus);
                if let Err(err) = panes[idx].write(&bytes) {
                    *footer_msg = write_error_footer(focus.0.label(), &err);
                    *error_raw_msg = footer_msg.clone();
                    *error_set_at = Some(Instant::now());
                }
                capture_line(focus.0, &bytes, input_buf, input_tx);
            }
        }
    }
    Ok(false)
}
