use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use anyhow::Result;
use crossterm::event::MouseEventKind;
use tokio::sync::mpsc;

use crate::native::layout::pane_id_index;
use crate::native::pane::{Pane, PaneId};
use crate::native::relay;
use crate::native::runtime_status::{pause_footer, record_relay_traffic, TrafficCounters};

pub(super) const SCROLL_LINES: usize = 5;

pub(super) fn drain_paused_writes(
    panes: &mut [Pane; 2],
    paused_writes: &mut VecDeque<(String, Vec<u8>)>,
    log_path: &std::path::Path,
    traffic: &mut TrafficCounters,
) -> bool {
    let mut dirty = false;
    while let Some((target, bytes)) = paused_writes.pop_front() {
        let byte_len = bytes.len() as u64;
        match write_to_target(panes, &target, &bytes) {
            Ok(()) => {
                record_relay_traffic(traffic, &target, byte_len, Instant::now());
                dirty = true;
            }
            Err(err) => {
                crate::relay_core::log_event(
                    log_path,
                    format!("relay_write_error target={target} error=\"{err}\""),
                );
                dirty = true;
            }
        }
    }
    dirty
}

#[allow(clippy::too_many_arguments)]
pub(super) fn drain_relay_writes(
    panes: &mut [Pane; 2],
    write_rx: &mut mpsc::Receiver<(String, Vec<u8>)>,
    relay_paused: bool,
    paused_writes: &mut VecDeque<(String, Vec<u8>)>,
    log_path: &std::path::Path,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
    traffic: &mut TrafficCounters,
) -> bool {
    let mut dirty = false;
    loop {
        match write_rx.try_recv() {
            Ok((target, bytes)) => {
                if relay_paused {
                    paused_writes.push_back((target, bytes));
                    *footer_msg = pause_footer(paused_writes.len());
                    *error_set_at = None;
                    dirty = true;
                } else {
                    dirty |= write_or_report(
                        panes,
                        &target,
                        &bytes,
                        log_path,
                        footer_msg,
                        error_set_at,
                        error_raw_msg,
                        traffic,
                    );
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    dirty
}

#[allow(clippy::too_many_arguments)]
fn write_or_report(
    panes: &mut [Pane; 2],
    target: &str,
    bytes: &[u8],
    log_path: &std::path::Path,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
    traffic: &mut TrafficCounters,
) -> bool {
    let byte_len = bytes.len() as u64;
    match write_to_target(panes, target, bytes) {
        Ok(()) => {
            record_relay_traffic(traffic, target, byte_len, Instant::now());
            true
        }
        Err(err) => {
            crate::relay_core::log_event(
                log_path,
                format!("relay_write_error target={target} error=\"{err}\""),
            );
            *footer_msg = write_error_footer(target, &err);
            *error_raw_msg = footer_msg.clone();
            *error_set_at = Some(Instant::now());
            true
        }
    }
}

pub(super) fn drain_relay_status(
    status_rx: &mut mpsc::Receiver<relay::RelayStatus>,
    relay_auto_stopped: &mut bool,
) -> bool {
    let mut dirty = false;
    loop {
        match status_rx.try_recv() {
            Ok(status) => {
                if *relay_auto_stopped != status.auto_stopped {
                    *relay_auto_stopped = status.auto_stopped;
                    dirty = true;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    dirty
}

fn write_to_target(panes: &mut [Pane; 2], target: &str, bytes: &[u8]) -> Result<()> {
    let idx = match target {
        "a" => 0,
        "b" => 1,
        _ => return Ok(()),
    };
    panes[idx].write(bytes)
}

pub(super) fn handle_mouse_wheel(
    panes: &mut [Pane; 2],
    pane: PaneId,
    kind: MouseEventKind,
    row: u16,
    col: u16,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
) {
    let idx = pane_id_index(pane);
    if panes[idx].agent == "codex" {
        if let Some(bytes) = mouse_wheel_bytes(kind, row, col) {
            if let Err(err) = panes[idx].write(&bytes) {
                *footer_msg = write_error_footer(pane.label(), &err);
                *error_set_at = Some(Instant::now());
            }
        }
        return;
    }

    match kind {
        MouseEventKind::ScrollUp => panes[idx].scroll_up(SCROLL_LINES),
        MouseEventKind::ScrollDown => panes[idx].scroll_down(SCROLL_LINES),
        _ => {}
    }
}

pub(super) fn handle_screen_scroll(
    panes: &mut [Pane; 2],
    pane: PaneId,
    action: &crate::native::input::GlobalAction,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
) {
    let idx = pane_id_index(pane);
    if panes[idx].agent == "codex" {
        if let Some(bytes) = codex_screen_scroll_bytes(action) {
            if let Err(err) = panes[idx].write(bytes) {
                *footer_msg = write_error_footer(pane.label(), &err);
                *error_raw_msg = footer_msg.clone();
                *error_set_at = Some(Instant::now());
            }
        }
        return;
    }

    match action {
        crate::native::input::GlobalAction::ScrollUp => panes[idx].scroll_up(SCROLL_LINES),
        crate::native::input::GlobalAction::ScrollDown => panes[idx].scroll_down(SCROLL_LINES),
        _ => {}
    }
}

pub(super) fn codex_screen_scroll_bytes(
    action: &crate::native::input::GlobalAction,
) -> Option<&'static [u8]> {
    match action {
        crate::native::input::GlobalAction::ScrollUp => Some(b"\x1b[5~"),
        crate::native::input::GlobalAction::ScrollDown => Some(b"\x1b[6~"),
        _ => None,
    }
}

pub(super) fn mouse_wheel_bytes(kind: MouseEventKind, row: u16, col: u16) -> Option<Vec<u8>> {
    let button = match kind {
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        _ => return None,
    };
    Some(format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes())
}

pub(super) fn write_error_footer(target: &str, err: &dyn std::fmt::Display) -> String {
    format!(" relay write failed for pane {target} · {err} ")
}

pub(super) fn capture_line(
    pane: PaneId,
    bytes: &[u8],
    buf: &mut HashMap<PaneId, Vec<u8>>,
    input_tx: &mpsc::Sender<(String, String)>,
) {
    let entry = buf.entry(pane).or_default();
    for &b in bytes {
        if b == b'\r' || b == b'\n' {
            if !entry.is_empty() {
                let line = std::mem::take(entry);
                if let Ok(text) = String::from_utf8(line) {
                    let pane_label = pane.label().to_string();
                    let _ = input_tx.try_send((pane_label, text));
                }
            }
        } else {
            entry.push(b);
        }
    }
}
