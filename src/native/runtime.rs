//! Native two-pane runtime: owns Pane A and Pane B PTYs, draws them with
//! ratatui, forwards keys to whichever pane has focus, runs the Claude Stop
//! hook HTTP server, and drives the in-process relay loop. The runtime
//! process is the cduo session — there is no background daemon.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::cli::{Agent, SplitLayout};
use crate::hook::{self, HookEvent};
use crate::native::access::{agent_args, agent_program, AccessMode};
use crate::native::footer::{mode_glyph, pingpong_dot, queue_gauge_glyph, uptime_label};
use crate::native::input::{classify_key, key_to_bytes, GlobalAction};
use crate::native::layout::{
    focus_index, pane_id_index, pane_layouts_for_view, resize_panes_for_view, split_label,
    toggle_split,
};
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::relay;
use crate::native::render::draw;
use crate::native::selection::{
    copy_to_clipboard_osc52, mouse_cell, mouse_cell_in_pane_clamped, mouse_pane, selected_text,
    MouseSelection,
};
use crate::native::ui::pane_pty_size;

const FRAME_BUDGET_MS: u64 = 16;
const POLL_INTERVAL_MS: u64 = 8;
const SCROLL_LINES: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct RuntimeOptions {
    pub agent_a: Agent,
    pub agent_b: Agent,
    pub split: SplitLayout,
    pub yolo: bool,
    pub full_access: bool,
    /// Reserved for future "always create a new session" semantics; native
    /// mode currently spawns a fresh session every time so this is a no-op.
    #[allow(dead_code)]
    pub new_session: bool,
}

pub async fn run(opts: RuntimeOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("get current dir")?;

    // Validate flags before allocating anything else.
    AccessMode::from_flags(opts.yolo, opts.full_access)?;

    let hook_listener = bind_hook_listener(preferred_hook_port()).await?;
    let hook_port = hook_listener
        .local_addr()
        .context("read hook listener address")?
        .port();
    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(64);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(64);
    let (write_tx, write_rx) = mpsc::channel::<(String, Vec<u8>)>(64);
    let (control_tx, control_rx) = mpsc::channel::<relay::RelayControl>(64);
    let (status_tx, status_rx) = mpsc::channel::<relay::RelayStatus>(16);
    if let Ok(prefix) = std::env::var("CDUO_RELAY_PREFIX") {
        if !prefix.is_empty() {
            let _ = control_tx.try_send(relay::RelayControl::SetPrefix(Some(prefix)));
        }
    }

    tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            hook::run_hook_server_on_listener(hook_listener, shutdown_rx, hook_tx).await;
        }
    });

    // Per-session log file under the platform state directory.
    let log_path = native_log_path()?;

    let pane_agents: HashMap<String, String> = HashMap::from([
        ("a".to_string(), agent_program(opts.agent_a).to_string()),
        ("b".to_string(), agent_program(opts.agent_b).to_string()),
    ]);

    let started_at = chrono::Utc::now();

    tokio::spawn(relay::run(relay::RelayInputs {
        cwd: cwd.clone(),
        started_at,
        log_path: log_path.clone(),
        pane_agents,
        hook_rx,
        control_rx,
        input_rx,
        write_tx,
        status_tx: Some(status_tx),
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    let channels = RuntimeChannels {
        input_tx,
        control_tx,
        write_rx,
        status_rx,
    };
    let join =
        tokio::task::spawn_blocking(move || run_blocking(opts, cwd, hook_port, log_path, channels));
    let result = join.await.context("native runtime join")?;

    let _ = shutdown_tx.send(());
    result
}

fn native_log_path() -> Result<PathBuf> {
    let dir = crate::session::get_state_root().join("native");
    std::fs::create_dir_all(&dir).ok();
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    Ok(dir.join(format!("session-{stamp}.log")))
}

async fn bind_hook_listener(start: u16) -> Result<TcpListener> {
    let mut last_port = start;
    for port in candidate_hook_ports(start) {
        last_port = port;
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return Ok(listener);
        }
    }
    anyhow::bail!("No available port found in range {start}-{last_port}")
}

fn candidate_hook_ports(start: u16) -> impl Iterator<Item = u16> {
    (0..100).filter_map(move |offset| start.checked_add(offset))
}

fn preferred_hook_port() -> u16 {
    std::env::var("CDUO_PORT")
        .or_else(|_| std::env::var("PORT"))
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(53333)
}

fn run_blocking(
    opts: RuntimeOptions,
    cwd: PathBuf,
    hook_port: u16,
    log_path: PathBuf,
    channels: RuntimeChannels,
) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).context("enter alt screen")?;

    let result = ui_loop(opts, &cwd, hook_port, &log_path, channels);

    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

fn ui_loop(
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
    } = channels;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let initial = terminal.size()?;
    let (pane_cols, pane_rows) = pane_pty_size(initial.width, initial.height, opts.split);
    let port_str = hook_port.to_string();
    let mode = AccessMode::from_flags(opts.yolo, opts.full_access)?;
    let mut split = opts.split;

    let pane_a = Pane::spawn(
        PaneId::A,
        agent_program(opts.agent_a),
        agent_args(opts.agent_a, mode),
        cwd,
        pane_cols,
        pane_rows,
        &[
            ("TERMINAL_ID", "a"),
            ("ORCHESTRATION_PORT", port_str.as_str()),
        ],
    )?;
    let pane_b = Pane::spawn(
        PaneId::B,
        agent_program(opts.agent_b),
        agent_args(opts.agent_b, mode),
        cwd,
        pane_cols,
        pane_rows,
        &[
            ("TERMINAL_ID", "b"),
            ("ORCHESTRATION_PORT", port_str.as_str()),
        ],
    )?;

    let mut panes: [Pane; 2] = [pane_a, pane_b];
    let mut focus = Focus(PaneId::A);
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    let runtime_start = Instant::now();
    let mut dirty = true;
    let mut footer_msg = format!(
        " hook:{}  · Ctrl-Y: broadcast  · Ctrl-W: focus  · Ctrl-P: pause relay  · Ctrl-L: split  · drag: copy  · PageUp/PageDown: scroll  · Ctrl-Q: quit ",
        hook_port,
    );
    let default_footer_msg = footer_msg.clone();
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

    // Per-pane buffer that mirrors what we forwarded to the agent. On every
    // \r/\n we flush it as a (pane_id, line) submission for the relay's codex
    // pending-prompt matching.
    let mut input_buf: HashMap<PaneId, Vec<u8>> = HashMap::new();

    let mut traffic = TrafficCounters {
        a_to_b_bytes: 0,
        b_to_a_bytes: 0,
        last_a_to_b_at: None,
        last_b_to_a_at: None,
        samples_a_to_b: std::collections::VecDeque::from(vec![0u64; 8]),
        samples_b_to_a: std::collections::VecDeque::from(vec![0u64; 8]),
        last_sample_at: Instant::now(),
    };

    'main: loop {
        if !relay_paused {
            drain_paused_writes(&mut panes, &mut paused_writes, log_path);
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
            || (a_to_b_enabled && b_to_a_enabled)
            || error_set_at.is_some();
        if needs_tick && !dirty && last_frame.elapsed() >= Duration::from_millis(250) {
            dirty = true;
        }

        if dirty && last_frame.elapsed() >= Duration::from_millis(FRAME_BUDGET_MS) {
            let now = Instant::now();
            let elapsed = runtime_start.elapsed();

            if let Some(at) = error_set_at {
                match crate::native::footer::error_toast_fade(&error_raw_msg, at.elapsed()) {
                    Some(faded) => footer_msg = faded,
                    None => {
                        footer_msg = default_footer_msg.clone();
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
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if broadcast_input.is_some() && classify_key(key) == GlobalAction::Quit {
                        break 'main;
                    }
                    if let Some(buffer) = broadcast_input.as_mut() {
                        match handle_broadcast_key(key, buffer) {
                            BroadcastInputAction::Editing => {
                                footer_msg = broadcast_input_footer(buffer, runtime_start.elapsed());
                                error_set_at = None;
                            }
                            BroadcastInputAction::Cancel => {
                                broadcast_input = None;
                                footer_msg = default_footer_msg.clone();
                                error_set_at = None;
                            }
                            BroadcastInputAction::Submit(prompt) => {
                                broadcast_input = None;
                                send_broadcast_prompt(
                                    &mut panes,
                                    &prompt,
                                    &mut input_buf,
                                    &input_tx,
                                    &mut footer_msg,
                                    &mut error_set_at,
                                    &mut error_raw_msg,
                                );
                                relay_paused = true;
                            }
                        }
                        dirty = true;
                        continue;
                    }
                    match classify_key(key) {
                        GlobalAction::Quit => break 'main,
                        GlobalAction::FocusNext => {
                            focus = focus.next();
                            if maximized.is_some() {
                                maximized = Some(focus.0);
                                let size = terminal.size()?;
                                resize_panes_for_view(
                                    &mut panes,
                                    size.width,
                                    size.height,
                                    split,
                                    maximized,
                                );
                            }
                            dirty = true;
                        }
                        GlobalAction::FocusPrev => {
                            focus = focus.prev();
                            if maximized.is_some() {
                                maximized = Some(focus.0);
                                let size = terminal.size()?;
                                resize_panes_for_view(
                                    &mut panes,
                                    size.width,
                                    size.height,
                                    split,
                                    maximized,
                                );
                            }
                            dirty = true;
                        }
                        GlobalAction::TogglePause => {
                            relay_paused = !relay_paused;
                            footer_msg = if relay_paused {
                                pause_footer(paused_writes.len())
                            } else {
                                default_footer_msg.clone()
                            };
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ToggleSplit => {
                            split = toggle_split(split);
                            let size = terminal.size()?;
                            resize_panes_for_view(
                                &mut panes,
                                size.width,
                                size.height,
                                split,
                                maximized,
                            );
                            footer_msg =
                                format!(" split: {} · Ctrl-L: toggle split ", split_label(split));
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ManualRelay => {
                            let pane_id = focus.0.label().to_string();
                            footer_msg = send_control_or_footer(
                                &control_tx,
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
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ClearRelayQueue => {
                            let cleared = clear_paused_writes(&mut paused_writes);
                            footer_msg =
                                format!(" relay queue cleared · dropped writes: {cleared} ");
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ToggleRelayAToB => {
                            a_to_b_enabled = !a_to_b_enabled;
                            footer_msg = send_control_or_footer(
                                &control_tx,
                                relay::RelayControl::SetRoute {
                                    source: "a".to_string(),
                                    target: "b".to_string(),
                                    enabled: a_to_b_enabled,
                                },
                                || route_footer("A→B", a_to_b_enabled),
                            );
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ToggleRelayBToA => {
                            b_to_a_enabled = !b_to_a_enabled;
                            footer_msg = send_control_or_footer(
                                &control_tx,
                                relay::RelayControl::SetRoute {
                                    source: "b".to_string(),
                                    target: "a".to_string(),
                                    enabled: b_to_a_enabled,
                                },
                                || route_footer("B→A", b_to_a_enabled),
                            );
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ShowRelayLog => {
                            footer_msg = recent_log_footer(log_path);
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ToggleFocusLayout => {
                            maximized = match maximized {
                                Some(active) if active == focus.0 => None,
                                _ => Some(focus.0),
                            };
                            let size = terminal.size()?;
                            resize_panes_for_view(
                                &mut panes,
                                size.width,
                                size.height,
                                split,
                                maximized,
                            );
                            footer_msg = match maximized {
                                Some(active) => {
                                    format!(
                                        " pane {} maximized · Ctrl-Z: restore ",
                                        active.label().to_uppercase()
                                    )
                                }
                                None => {
                                    " layout restored · Ctrl-Z: maximize focused pane ".to_string()
                                }
                            };
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::BroadcastInput => {
                            broadcast_input = Some(String::new());
                            footer_msg = broadcast_input_footer("", runtime_start.elapsed());
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ScrollUp => {
                            panes[focus_index(focus)].scroll_up(SCROLL_LINES);
                            dirty = true;
                        }
                        GlobalAction::ScrollDown => {
                            panes[focus_index(focus)].scroll_down(SCROLL_LINES);
                            dirty = true;
                        }
                        GlobalAction::Forward => {
                            if let Some(bytes) = key_to_bytes(key) {
                                let idx = focus_index(focus);
                                if let Err(err) = panes[idx].write(&bytes) {
                                    footer_msg = write_error_footer(focus.0.label(), &err);
                                    error_raw_msg = footer_msg.clone();
                                    error_set_at = Some(Instant::now());
                                }
                                capture_line(focus.0, &bytes, &mut input_buf, &input_tx);
                            }
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    resize_panes_for_view(&mut panes, cols, rows, split, maximized);
                    dirty = true;
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    let (layouts, _) = pane_layouts_for_view(area, split, maximized);
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some((pane, row, col)) =
                                mouse_cell(mouse.column, mouse.row, layouts)
                            {
                                focus = Focus(pane);
                                selection = Some(MouseSelection {
                                    pane,
                                    start_row: row,
                                    start_col: col,
                                    end_row: row,
                                    end_col: col,
                                });
                                footer_msg = default_footer_msg.clone();
                                error_set_at = None;
                                dirty = true;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(active) = selection.as_mut() {
                                if let Some((_, row, col)) = mouse_cell_in_pane_clamped(
                                    mouse.column,
                                    mouse.row,
                                    layouts,
                                    active.pane,
                                ) {
                                    active.end_row = row;
                                    active.end_col = col;
                                    dirty = true;
                                }
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if let Some(active) = selection.take() {
                                let pane_idx = pane_id_index(active.pane);
                                let text =
                                    selected_text(panes[pane_idx].parser.screen(), active.range());
                                if !text.is_empty() {
                                    copy_to_clipboard_osc52(&mut terminal, &text)?;
                                    footer_msg = format!(
                                        " copied {} chars from pane {} ",
                                        text.chars().count(),
                                        active.pane.label().to_uppercase()
                                    );
                                    error_set_at = None;
                                }
                                dirty = true;
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            if let Some((pane, row, col)) =
                                mouse_cell(mouse.column, mouse.row, layouts)
                            {
                                handle_mouse_wheel(
                                    &mut panes,
                                    pane,
                                    MouseEventKind::ScrollUp,
                                    row,
                                    col,
                                    &mut footer_msg,
                                    &mut error_set_at,
                                );
                                if error_set_at.is_some() {
                                    error_raw_msg = footer_msg.clone();
                                }
                            } else {
                                let pane =
                                    mouse_pane(mouse.column, mouse.row, layouts).unwrap_or(focus.0);
                                panes[pane_id_index(pane)].scroll_up(SCROLL_LINES);
                            }
                            dirty = true;
                        }
                        MouseEventKind::ScrollDown => {
                            if let Some((pane, row, col)) =
                                mouse_cell(mouse.column, mouse.row, layouts)
                            {
                                handle_mouse_wheel(
                                    &mut panes,
                                    pane,
                                    MouseEventKind::ScrollDown,
                                    row,
                                    col,
                                    &mut footer_msg,
                                    &mut error_set_at,
                                );
                                if error_set_at.is_some() {
                                    error_raw_msg = footer_msg.clone();
                                }
                            } else {
                                let pane =
                                    mouse_pane(mouse.column, mouse.row, layouts).unwrap_or(focus.0);
                                panes[pane_id_index(pane)].scroll_down(SCROLL_LINES);
                            }
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Surface child exit so the user can see it before quitting.
        for pane in panes.iter_mut() {
            if pane.child_exited() {
                footer_msg = format!(
                    " pane {} exited · Ctrl-Q to leave ",
                    pane.id.label().to_uppercase()
                );
                error_set_at = None;
                dirty = true;
            }
        }
    }

    for pane in panes.iter_mut() {
        pane.kill();
    }
    Ok(())
}

struct RuntimeChannels {
    input_tx: mpsc::Sender<(String, String)>,
    control_tx: mpsc::Sender<relay::RelayControl>,
    write_rx: mpsc::Receiver<(String, Vec<u8>)>,
    status_rx: mpsc::Receiver<relay::RelayStatus>,
}

fn drain_paused_writes(
    panes: &mut [Pane; 2],
    paused_writes: &mut VecDeque<(String, Vec<u8>)>,
    log_path: &std::path::Path,
) {
    while let Some((target, bytes)) = paused_writes.pop_front() {
        if let Err(err) = write_to_target(panes, &target, &bytes) {
            crate::relay_core::log_event(
                log_path,
                format!("relay_write_error target={target} error=\"{err}\""),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_relay_writes(
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
                    let byte_len = bytes.len() as u64;
                    let target_str = target.clone();
                    match write_to_target(panes, &target, &bytes) {
                        Ok(()) => {
                            match target_str.as_str() {
                                "b" => {
                                    traffic.a_to_b_bytes += byte_len;
                                    traffic.last_a_to_b_at = Some(Instant::now());
                                }
                                "a" => {
                                    traffic.b_to_a_bytes += byte_len;
                                    traffic.last_b_to_a_at = Some(Instant::now());
                                }
                                _ => {}
                            }
                        }
                        Err(err) => {
                            crate::relay_core::log_event(
                                log_path,
                                format!("relay_write_error target={target} error=\"{err}\""),
                            );
                            *footer_msg = write_error_footer(&target, &err);
                            *error_raw_msg = footer_msg.clone();
                            *error_set_at = Some(Instant::now());
                            dirty = true;
                        }
                    }
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    dirty
}

fn drain_relay_status(
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

fn handle_mouse_wheel(
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

fn mouse_wheel_bytes(kind: MouseEventKind, row: u16, col: u16) -> Option<Vec<u8>> {
    let button = match kind {
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        _ => return None,
    };
    Some(format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes())
}

#[derive(Debug, PartialEq, Eq)]
enum BroadcastInputAction {
    Editing,
    Cancel,
    Submit(String),
}

fn handle_broadcast_key(key: KeyEvent, buffer: &mut String) -> BroadcastInputAction {
    match key.code {
        KeyCode::Esc => BroadcastInputAction::Cancel,
        KeyCode::Char('y') | KeyCode::Char('Y')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            BroadcastInputAction::Cancel
        }
        KeyCode::Enter => {
            let prompt = buffer.trim().to_string();
            if prompt.is_empty() {
                BroadcastInputAction::Cancel
            } else {
                BroadcastInputAction::Submit(prompt)
            }
        }
        KeyCode::Backspace => {
            buffer.pop();
            BroadcastInputAction::Editing
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            buffer.push(c);
            BroadcastInputAction::Editing
        }
        _ => BroadcastInputAction::Editing,
    }
}

fn broadcast_prompt_bytes(prompt: &str) -> Vec<u8> {
    format!("{prompt}\r").into_bytes()
}

fn send_broadcast_prompt(
    panes: &mut [Pane; 2],
    prompt: &str,
    input_buf: &mut HashMap<PaneId, Vec<u8>>,
    input_tx: &mpsc::Sender<(String, String)>,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
) {
    let bytes = broadcast_prompt_bytes(prompt);
    let mut sent = 0;
    for pane in [PaneId::A, PaneId::B] {
        let idx = pane_id_index(pane);
        match panes[idx].write(&bytes) {
            Ok(()) => {
                capture_line(pane, &bytes, input_buf, input_tx);
                sent += 1;
            }
            Err(err) => {
                *footer_msg = write_error_footer(pane.label(), &err);
                *error_raw_msg = footer_msg.clone();
                *error_set_at = Some(Instant::now());
                return;
            }
        }
    }

    *footer_msg = format!(" broadcast sent to {sent} panes · relay paused · Ctrl-P: resume relay ");
    *error_set_at = None;
}

fn broadcast_input_footer(buffer: &str, elapsed: Duration) -> String {
    let caret = crate::native::footer::broadcast_caret_glyph(elapsed);
    format!(" broadcast> {buffer}{caret} · Enter: send · Esc: cancel ")
}

fn send_control_or_footer(
    control_tx: &mpsc::Sender<relay::RelayControl>,
    control: relay::RelayControl,
    success_footer: impl FnOnce() -> String,
) -> String {
    match control_tx.try_send(control) {
        Ok(()) => success_footer(),
        Err(err) => format!(" relay control unavailable · {err} "),
    }
}

fn write_error_footer(target: &str, err: &dyn std::fmt::Display) -> String {
    format!(" relay write failed for pane {target} · {err} ")
}

fn pause_footer(queued_writes: usize) -> String {
    format!(" relay paused · queued writes: {queued_writes} · Ctrl-P: resume ")
}

fn clear_paused_writes(paused_writes: &mut VecDeque<(String, Vec<u8>)>) -> usize {
    let cleared = paused_writes.len();
    paused_writes.clear();
    cleared
}

fn route_footer(route: &str, enabled: bool) -> String {
    let state = if enabled { "ON" } else { "OFF" };
    let route = match route {
        "A→B" => "A=>B",
        "B→A" => "B=>A",
        other => other,
    };
    format!(" route[{route}:{state}] · Ctrl-1: A=>B · Ctrl-2: B=>A ")
}

struct TrafficCounters {
    a_to_b_bytes: u64,
    b_to_a_bytes: u64,
    last_a_to_b_at: Option<Instant>,
    last_b_to_a_at: Option<Instant>,
    samples_a_to_b: std::collections::VecDeque<u64>,
    samples_b_to_a: std::collections::VecDeque<u64>,
    last_sample_at: Instant,
}

struct RelayStatusView<'a> {
    message: &'a str,
    relay_paused: bool,
    queued_writes: usize,
    a_to_b_enabled: bool,
    b_to_a_enabled: bool,
    relay_auto_stopped: bool,
    heartbeat: bool,
    elapsed: Duration,
    traffic: &'a TrafficCounters,
    now: Instant,
}

fn footer_with_relay_status(view: RelayStatusView<'_>) -> String {
    use crate::native::footer::{
        activity_dot, direction_arrow, stop_warn_glyph, traffic_sparkline,
    };
    let RelayStatusView {
        message,
        relay_paused,
        queued_writes,
        a_to_b_enabled,
        b_to_a_enabled,
        relay_auto_stopped,
        heartbeat,
        elapsed,
        traffic,
        now,
    } = view;

    let mode = if relay_auto_stopped {
        "STOP"
    } else if relay_paused {
        "PAUSE"
    } else {
        "ON"
    };
    let glyph = mode_glyph(mode);
    let warn = if relay_auto_stopped { stop_warn_glyph(elapsed) } else { "" };
    let pulse = if relay_paused && !relay_auto_stopped {
        if heartbeat { " ●" } else { " ○" }
    } else {
        ""
    };
    let gauge = queue_gauge_glyph(queued_writes);

    let pulse_a_to_b = traffic
        .last_a_to_b_at
        .map(|t| now.duration_since(t) < Duration::from_millis(200))
        .unwrap_or(false);
    let pulse_b_to_a = traffic
        .last_b_to_a_at
        .map(|t| now.duration_since(t) < Duration::from_millis(200))
        .unwrap_or(false);
    let arrow_ab = direction_arrow(a_to_b_enabled, pulse_a_to_b);
    let arrow_ba = direction_arrow(b_to_a_enabled, pulse_b_to_a);
    let spark_ab_vec: Vec<u64> = traffic.samples_a_to_b.iter().copied().collect();
    let spark_ba_vec: Vec<u64> = traffic.samples_b_to_a.iter().copied().collect();
    let spark_ab = traffic_sparkline(&spark_ab_vec);
    let spark_ba = traffic_sparkline(&spark_ba_vec);
    let act_a = activity_dot(*traffic.samples_b_to_a.back().unwrap_or(&0));
    let act_b = activity_dot(*traffic.samples_a_to_b.back().unwrap_or(&0));

    let routes = if !relay_paused && !relay_auto_stopped && a_to_b_enabled && b_to_a_enabled {
        let pp = pingpong_dot(elapsed);
        format!("A{act_a} {spark_ba} {arrow_ba} {pp} {arrow_ab} {spark_ab} {act_b}B")
    } else {
        format!("A{act_a} {arrow_ba} | {arrow_ab} {act_b}B")
    };
    let uptime = uptime_label(elapsed);
    format!(
        " {glyph}{warn} relay[{mode}]{pulse} · q[{queued_writes}]{gauge} · {routes} · up {uptime} · {}",
        message.trim()
    )
}

fn recent_log_footer(log_path: &std::path::Path) -> String {
    let Ok(contents) = std::fs::read_to_string(log_path) else {
        return " relay log unavailable ".to_string();
    };
    let line = contents
        .lines()
        .rev()
        .find(|line| {
            line.contains("publish")
                || line.contains("dedup")
                || line.contains("deliver")
                || line.contains("route")
                || line.contains("manual")
        })
        .unwrap_or("relay log empty");
    let mut msg = format!(" relay log · {line} ");
    msg.truncate(220);
    msg
}

/// Mirror forwarded keystrokes for the focused pane and emit the buffered text
/// as a (pane_id, line) submission whenever a CR or LF byte goes through. The
/// relay loop uses these to match codex transcripts to their owning pane.
fn capture_line(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn preferred_hook_port_defaults_when_env_missing_or_invalid() {
        let _guard = env_lock();
        std::env::remove_var("CDUO_PORT");
        std::env::set_var("PORT", "not-a-port");
        assert_eq!(preferred_hook_port(), 53333);
        std::env::remove_var("PORT");
    }

    #[test]
    fn preferred_hook_port_accepts_cduo_port_over_port() {
        let _guard = env_lock();
        std::env::set_var("PORT", "12345");
        std::env::set_var("CDUO_PORT", "23456");
        assert_eq!(preferred_hook_port(), 23456);
        std::env::remove_var("CDUO_PORT");
        std::env::remove_var("PORT");
    }

    #[test]
    fn candidate_hook_ports_stops_at_u16_max_without_overflow() {
        let ports: Vec<u16> = candidate_hook_ports(u16::MAX - 1).collect();
        assert_eq!(ports, vec![u16::MAX - 1, u16::MAX]);
    }

    #[tokio::test]
    async fn bind_hook_listener_skips_busy_port_and_keeps_listener_bound() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let busy_port = busy.local_addr().unwrap().port();

        let listener = bind_hook_listener(busy_port).await.unwrap();
        let selected_port = listener.local_addr().unwrap().port();

        assert_ne!(selected_port, busy_port);
        assert!(TcpListener::bind(("127.0.0.1", selected_port))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn capture_line_emits_on_cr() {
        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, b"hi", &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        capture_line(PaneId::A, b"\r", &mut buf, &tx);

        let (pane, text) = rx.try_recv().unwrap();
        assert_eq!(pane, "a");
        assert_eq!(text, "hi");
    }

    #[tokio::test]
    async fn capture_line_separates_panes() {
        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, b"alpha", &mut buf, &tx);
        capture_line(PaneId::B, b"beta\r", &mut buf, &tx);
        capture_line(PaneId::A, b"\r", &mut buf, &tx);

        let mut got: Vec<(String, String)> = Vec::new();
        while let Ok(item) = rx.try_recv() {
            got.push(item);
        }
        assert_eq!(
            got,
            vec![
                ("b".to_string(), "beta".to_string()),
                ("a".to_string(), "alpha".to_string()),
            ]
        );
    }

    #[test]
    fn clear_paused_writes_drops_all_queued_relay_bytes() {
        let mut queue = VecDeque::from([
            ("a".to_string(), b"one".to_vec()),
            ("b".to_string(), b"two".to_vec()),
        ]);

        assert_eq!(clear_paused_writes(&mut queue), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn mouse_wheel_bytes_use_sgr_coordinates() {
        assert_eq!(
            mouse_wheel_bytes(MouseEventKind::ScrollUp, 2, 3).unwrap(),
            b"\x1b[<64;4;3M"
        );
        assert_eq!(
            mouse_wheel_bytes(MouseEventKind::ScrollDown, 2, 3).unwrap(),
            b"\x1b[<65;4;3M"
        );
        assert!(mouse_wheel_bytes(MouseEventKind::Moved, 2, 3).is_none());
    }

    #[test]
    fn broadcast_key_buffer_edits_and_submits() {
        let mut buffer = String::new();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "hi");

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "h");

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut buffer
            ),
            BroadcastInputAction::Submit("h".to_string())
        );
    }

    #[test]
    fn broadcast_key_ignores_control_chars_and_cancels() {
        let mut buffer = "keep".to_string();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut buffer
            ),
            BroadcastInputAction::Editing
        );
        assert_eq!(buffer, "keep");
        assert_eq!(
            handle_broadcast_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut buffer),
            BroadcastInputAction::Cancel
        );
    }

    #[test]
    fn broadcast_key_ctrl_y_cancels_mode() {
        let mut buffer = "draft".to_string();

        assert_eq!(
            handle_broadcast_key(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
                &mut buffer
            ),
            BroadcastInputAction::Cancel
        );
        assert_eq!(buffer, "draft");
    }

    #[test]
    fn broadcast_prompt_bytes_add_enter_and_capture_both_panes() {
        let bytes = broadcast_prompt_bytes("same prompt");
        assert_eq!(bytes, b"same prompt\r");

        let mut buf: HashMap<PaneId, Vec<u8>> = HashMap::new();
        let (tx, mut rx) = mpsc::channel::<(String, String)>(8);

        capture_line(PaneId::A, &bytes, &mut buf, &tx);
        capture_line(PaneId::B, &bytes, &mut buf, &tx);

        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        assert_eq!(first, ("a".to_string(), "same prompt".to_string()));
        assert_eq!(second, ("b".to_string(), "same prompt".to_string()));
    }

    #[test]
    fn broadcast_footer_names_controls() {
        let footer = broadcast_input_footer("compare this", Duration::from_secs(0));

        assert!(footer.contains("broadcast> compare this"));
        assert!(footer.contains("Enter"));
        assert!(footer.contains("Esc"));
    }

    #[test]
    fn send_control_or_footer_reports_closed_control_channel() {
        let (tx, rx) = mpsc::channel::<relay::RelayControl>(1);
        drop(rx);

        let footer = send_control_or_footer(
            &tx,
            relay::RelayControl::SetPrefix(Some("prefix".to_string())),
            || "success".to_string(),
        );

        assert!(footer.contains("relay control unavailable"));
    }

    #[test]
    fn write_error_footer_names_target_pane() {
        let footer = write_error_footer("a", &std::io::Error::other("closed"));
        assert!(footer.contains("pane a"));
        assert!(footer.contains("closed"));
    }

    fn test_traffic() -> TrafficCounters {
        TrafficCounters {
            a_to_b_bytes: 0,
            b_to_a_bytes: 0,
            last_a_to_b_at: None,
            last_b_to_a_at: None,
            samples_a_to_b: std::collections::VecDeque::from(vec![0u64; 8]),
            samples_b_to_a: std::collections::VecDeque::from(vec![0u64; 8]),
            last_sample_at: Instant::now(),
        }
    }

    #[test]
    fn footer_status_shows_relay_mode_queue_and_routes() {
        let traffic = test_traffic();
        let footer = footer_with_relay_status(RelayStatusView {
            message: "ready",
            relay_paused: true,
            queued_writes: 3,
            a_to_b_enabled: false,
            b_to_a_enabled: true,
            relay_auto_stopped: false,
            heartbeat: true,
            elapsed: Duration::from_secs(0),
            traffic: &traffic,
            now: Instant::now(),
        });

        assert!(footer.contains("⏸"));
        assert!(footer.contains("relay[PAUSE] ●"));
        assert!(footer.contains("q[3] ▃"));
        assert!(footer.contains("─x─")); // a_to_b disabled
        assert!(footer.contains("─▶─")); // b_to_a enabled
        assert!(footer.contains("up 00:00"));
        assert!(footer.ends_with("ready"));
    }

    #[test]
    fn footer_status_shows_stopped_relay_over_pause_state() {
        let traffic = test_traffic();
        let footer = footer_with_relay_status(RelayStatusView {
            message: "ready",
            relay_paused: true,
            queued_writes: 3,
            a_to_b_enabled: true,
            b_to_a_enabled: true,
            relay_auto_stopped: true,
            heartbeat: true,
            elapsed: Duration::from_secs(0),
            traffic: &traffic,
            now: Instant::now(),
        });

        assert!(footer.contains("⏹"));
        assert!(footer.contains("relay[STOP]"));
        assert!(!footer.contains("relay[PAUSE]"));
        assert!(!footer.contains('○'));
        assert!(footer.contains("up 00:00"));
    }

    #[test]
    fn route_footer_uses_ascii_indicator() {
        let footer = route_footer("A→B", false);

        assert!(footer.contains("route[A=>B:OFF]"));
        assert!(!footer.contains("A→B"));
    }
}
