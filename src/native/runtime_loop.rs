use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::native::access::{agent_args, agent_program, AccessMode};
use crate::native::layout::resize_panes_for_view;
use crate::native::pane::{Focus, Pane, PaneId, PaneSpawnOptions};
use crate::native::render::draw;
use crate::native::runtime::RuntimeOptions;
use crate::native::runtime_events::handle_key_event;
use crate::native::runtime_io::*;
use crate::native::runtime_loop_support::*;
use crate::native::runtime_mouse_events::handle_mouse_event;
use crate::native::runtime_status::{footer_with_relay_status, log_ticker_footer, RelayStatusView};
use crate::native::selection::MouseSelection;
use crate::native::ui::pane_pty_size;

const FRAME_BUDGET_MS: u64 = 16;
const POLL_INTERVAL_MS: u64 = 8;

pub(super) fn ui_loop(
    opts: RuntimeOptions,
    cwd: &std::path::Path,
    hook_port: u16,
    log_path: &std::path::Path,
    channels: RuntimeChannels,
) -> Result<()> {
    let RuntimeChannels {
        input_tx,
        control_tx,
        mut write_rx,
        mut status_rx,
        mut hook_ping_rx,
    } = channels;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let initial = terminal.size()?;
    let mut footer_width = initial.width;
    let (pane_cols, pane_rows) = pane_pty_size(initial.width, initial.height, opts.split);
    let port_str = hook_port.to_string();
    let mode = AccessMode::from_flags(opts.yolo, opts.full_access)?;
    let mut split = opts.split;

    let pane_a_env = pane_env(
        "a",
        port_str.as_str(),
        opts.session_name.as_deref(),
        opts.role_a.as_deref(),
    );
    let pane_b_env = pane_env(
        "b",
        port_str.as_str(),
        opts.session_name.as_deref(),
        opts.role_b.as_deref(),
    );

    let pane_a = Pane::spawn(PaneSpawnOptions {
        id: PaneId::A,
        agent: agent_program(opts.agent_a),
        args: agent_args(opts.agent_a, mode),
        cwd,
        cols: pane_cols,
        rows: pane_rows,
        env: &pane_a_env,
        role: opts.role_a.clone(),
        session_name: opts.session_name.clone(),
    })?;
    let pane_b = Pane::spawn(PaneSpawnOptions {
        id: PaneId::B,
        agent: agent_program(opts.agent_b),
        args: agent_args(opts.agent_b, mode),
        cwd,
        cols: pane_cols,
        rows: pane_rows,
        env: &pane_b_env,
        role: opts.role_b.clone(),
        session_name: opts.session_name.clone(),
    })?;

    let mut panes: [Pane; 2] = [pane_a, pane_b];
    let mut focus = Focus(PaneId::A);
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    let runtime_start = Instant::now();
    let mut dirty = true;
    let mut last_hook_at: Option<Instant> = None;
    let mut footer_msg = default_footer_message(hook_port, "");
    let mut error_set_at: Option<Instant> = None;
    let mut error_raw_msg: String = String::new();
    let mut selection: Option<MouseSelection> = None;
    let mut relay_paused = false;
    let mut paused_writes: VecDeque<(String, Vec<u8>)> = VecDeque::new();
    let mut a_to_b_enabled = true;
    let mut b_to_a_enabled = true;
    let mut relay_auto_stopped = false;
    let mut maximized: Option<PaneId> = None;
    let mut broadcast_input: Option<String> = None;
    let mut metadata_input: Option<String> = None;
    let mut log_ticker_on = false;
    let mut log_ticker_offset: usize = 0;
    let mut log_ticker_last_tick = Instant::now();
    let mut log_ticker_line: String = String::new();

    let mut input_buf: HashMap<PaneId, Vec<u8>> = HashMap::new();

    let mut traffic = empty_traffic_counters();

    'main: loop {
        // Drain hook pings — non-blocking, lossy.
        while hook_ping_rx.try_recv().is_ok() {
            last_hook_at = Some(Instant::now());
            dirty = true;
        }

        let dot = crate::native::footer::hook_ping_glyph(last_hook_at.map(|t| t.elapsed()));
        let default_footer_msg = default_footer_message(hook_port, dot);

        if !relay_paused
            && drain_paused_writes(&mut panes, &mut paused_writes, log_path, &mut traffic)
        {
            dirty = true;
        }

        if drain_relay_writes(
            &mut panes,
            &mut write_rx,
            relay_paused,
            &mut paused_writes,
            log_path,
            &mut footer_msg,
            &mut error_set_at,
            &mut error_raw_msg,
            &mut traffic,
        ) {
            dirty = true;
        }

        if drain_relay_status(&mut status_rx, &mut relay_auto_stopped) {
            dirty = true;
        }

        let mut produced = false;
        for pane in panes.iter_mut() {
            if pane.drain_into_parser() {
                produced = true;
            }
        }
        if produced {
            dirty = true;
        }

        let needs_tick = relay_paused
            || relay_auto_stopped
            || broadcast_input.is_some()
            || metadata_input.is_some()
            || (a_to_b_enabled && b_to_a_enabled)
            || error_set_at.is_some()
            || last_hook_at.is_some()
            || log_ticker_on;
        if needs_tick && !dirty && last_frame.elapsed() >= Duration::from_millis(250) {
            dirty = true;
        }

        if log_ticker_on {
            if log_ticker_last_tick.elapsed() >= Duration::from_millis(150) {
                log_ticker_offset = log_ticker_offset.wrapping_add(1);
                log_ticker_last_tick = Instant::now();
                // Refresh cached line on each tick.
                log_ticker_line = std::fs::read_to_string(log_path)
                    .ok()
                    .and_then(|c| c.lines().last().map(str::to_string))
                    .unwrap_or_default();
                dirty = true;
            }
            if error_set_at.is_none() {
                footer_msg = log_ticker_footer(&log_ticker_line, log_ticker_offset, footer_width);
            }
        }

        if dirty && last_frame.elapsed() >= Duration::from_millis(FRAME_BUDGET_MS) {
            let now = Instant::now();
            let elapsed = runtime_start.elapsed();

            if let Some(at) = error_set_at {
                match crate::native::footer::error_toast_fade(&error_raw_msg, at.elapsed()) {
                    Some(faded) => footer_msg = faded,
                    None => {
                        footer_msg = if log_ticker_on {
                            log_ticker_footer(&log_ticker_line, log_ticker_offset, footer_width)
                        } else {
                            default_footer_msg.clone()
                        };
                        error_set_at = None;
                    }
                }
            }
            let heartbeat = relay_paused && (elapsed.as_millis() / 500) % 2 == 0;
            let footer = footer_with_relay_status(RelayStatusView {
                message: &footer_msg,
                relay_paused,
                queued_writes: paused_writes.len(),
                a_to_b_enabled,
                b_to_a_enabled,
                relay_auto_stopped,
                heartbeat,
                elapsed,
                traffic: &traffic,
                now,
            });
            terminal.draw(|frame| {
                draw(frame, &panes, focus, &footer, selection, split, maximized);
            })?;
            last_frame = Instant::now();
            dirty = false;
        }

        if traffic.last_sample_at.elapsed() >= Duration::from_secs(1) {
            traffic.samples_a_to_b.pop_front();
            traffic.samples_a_to_b.push_back(traffic.a_to_b_bytes);
            traffic.samples_b_to_a.pop_front();
            traffic.samples_b_to_a.push_back(traffic.b_to_a_bytes);
            traffic.a_to_b_bytes = 0;
            traffic.b_to_a_bytes = 0;
            traffic.last_sample_at = Instant::now();
            dirty = true;
        }

        if event::poll(Duration::from_millis(POLL_INTERVAL_MS))? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key_event(
                        key,
                        &mut terminal,
                        &mut panes,
                        &mut focus,
                        &mut split,
                        &mut maximized,
                        &mut broadcast_input,
                        &mut metadata_input,
                        &mut input_buf,
                        &input_tx,
                        &control_tx,
                        &mut relay_paused,
                        &mut relay_auto_stopped,
                        &mut paused_writes,
                        &mut a_to_b_enabled,
                        &mut b_to_a_enabled,
                        &mut log_ticker_on,
                        &mut log_ticker_offset,
                        &mut log_ticker_last_tick,
                        &mut log_ticker_line,
                        runtime_start,
                        log_path,
                        &default_footer_msg,
                        &mut footer_msg,
                        &mut error_set_at,
                        &mut error_raw_msg,
                        &mut dirty,
                    )? {
                        break 'main;
                    }
                }
                Event::Resize(cols, rows) => {
                    footer_width = cols;
                    resize_panes_for_view(&mut panes, cols, rows, split, maximized);
                    dirty = true;
                }
                Event::Mouse(mouse) => {
                    handle_mouse_event(
                        mouse,
                        &mut terminal,
                        &mut panes,
                        &mut focus,
                        split,
                        maximized,
                        &mut selection,
                        &default_footer_msg,
                        &mut footer_msg,
                        &mut error_set_at,
                        &mut error_raw_msg,
                        &mut dirty,
                    )?;
                }
                _ => {}
            }
        }

        surface_child_exit(&mut panes, &mut footer_msg, &mut error_set_at, &mut dirty);
    }

    for pane in panes.iter_mut() {
        pane.kill();
    }
    Ok(())
}
