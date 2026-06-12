//! Native two-pane runtime: owns Pane A and Pane B PTYs, draws them with
//! ratatui, forwards keys to whichever pane has focus, runs the Claude Stop
//! hook HTTP server, and drives the in-process relay loop. The runtime
//! process is the cduo session — there is no background daemon.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
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
use crate::native::input::{classify_key, key_to_bytes, GlobalAction};
use crate::native::layout::{
    focus_index, pane_id_index, pane_layouts_for_view, resize_panes_for_view, split_label,
    toggle_split,
};
use crate::native::pane::{Focus, Pane, PaneId, PaneSpawnOptions};
use crate::native::relay;
use crate::native::render::draw;
use crate::native::runtime_metadata::*;
use crate::native::runtime_status::*;
use crate::native::selection::{
    copy_to_clipboard_osc52, mouse_cell, mouse_cell_in_pane_clamped, mouse_pane, selected_text,
    MouseSelection,
};
use crate::native::ui::pane_pty_size;

const FRAME_BUDGET_MS: u64 = 16;
const POLL_INTERVAL_MS: u64 = 8;
const SCROLL_LINES: usize = 5;

#[derive(Debug, Clone)]
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
    pub session_name: Option<String>,
    pub role_a: Option<String>,
    pub role_b: Option<String>,
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
    let (hook_ping_tx, hook_ping_rx) = mpsc::channel::<()>(64);
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
            hook::run_hook_server_on_listener(
                hook_listener,
                shutdown_rx,
                hook_tx,
                Some(hook_ping_tx),
            )
            .await;
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
        hook_ping_rx,
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
    let mut footer_msg = format!(
        " hook:{} ·  · Ctrl-Y: broadcast  · Ctrl-N: names  · Ctrl-W: focus  · Ctrl-P: pause relay  · Ctrl-L: split  · drag: copy  · PageUp/PageDown: scroll  · Ctrl-Q: quit ",
        hook_port,
    );
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
        // Drain hook pings — non-blocking, lossy.
        while hook_ping_rx.try_recv().is_ok() {
            last_hook_at = Some(Instant::now());
            dirty = true;
        }

        let dot = crate::native::footer::hook_ping_glyph(last_hook_at.map(|t| t.elapsed()));
        let default_footer_msg = format!(
            " hook:{}{dot}  · Ctrl-Y: broadcast  · Ctrl-N: names  · Ctrl-W: focus  · Ctrl-P: pause relay  · Ctrl-L: split  · drag: copy  · PageUp/PageDown: scroll  · Ctrl-Q: quit ",
            hook_port,
        );

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
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if (broadcast_input.is_some() || metadata_input.is_some())
                        && classify_key(key) == GlobalAction::Quit
                    {
                        break 'main;
                    }
                    if let Some(buffer) = metadata_input.as_mut() {
                        match handle_metadata_key(key, buffer) {
                            MetadataInputAction::Editing => {
                                footer_msg = metadata_input_footer(buffer);
                                error_set_at = None;
                            }
                            MetadataInputAction::Cancel => {
                                metadata_input = None;
                                footer_msg = default_footer_msg.clone();
                                error_set_at = None;
                            }
                            MetadataInputAction::Submit(input) => {
                                metadata_input = None;
                                footer_msg = match parse_metadata_update(&input) {
                                    Ok(update) => apply_metadata_update(&mut panes, update),
                                    Err(err) => format!(" metadata unchanged · {err} "),
                                };
                                error_set_at = None;
                            }
                        }
                        dirty = true;
                        continue;
                    }
                    if let Some(buffer) = broadcast_input.as_mut() {
                        match handle_broadcast_key(key, buffer) {
                            BroadcastInputAction::Editing => {
                                footer_msg =
                                    broadcast_input_footer(buffer, runtime_start.elapsed());
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
                            footer_msg = if relay_auto_stopped {
                                relay_auto_stopped = false;
                                relay_paused = false;
                                send_control_or_footer(
                                    &control_tx,
                                    relay::RelayControl::ResetStop,
                                    relay_reset_footer,
                                )
                            } else if relay_paused {
                                relay_paused = false;
                                default_footer_msg.clone()
                            } else {
                                relay_paused = true;
                                pause_footer(paused_writes.len())
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
                        GlobalAction::EditMetadata => {
                            metadata_input = Some(current_metadata_input(&panes));
                            footer_msg =
                                metadata_input_footer(metadata_input.as_deref().unwrap_or(""));
                            error_set_at = None;
                            dirty = true;
                        }
                        GlobalAction::ScrollUp => {
                            handle_screen_scroll(
                                &mut panes,
                                focus.0,
                                GlobalAction::ScrollUp,
                                &mut footer_msg,
                                &mut error_set_at,
                                &mut error_raw_msg,
                            );
                            dirty = true;
                        }
                        GlobalAction::ScrollDown => {
                            handle_screen_scroll(
                                &mut panes,
                                focus.0,
                                GlobalAction::ScrollDown,
                                &mut footer_msg,
                                &mut error_set_at,
                                &mut error_raw_msg,
                            );
                            dirty = true;
                        }
                        GlobalAction::ToggleLogTicker => {
                            log_ticker_on = !log_ticker_on;
                            log_ticker_offset = 0;
                            log_ticker_last_tick = Instant::now();
                            if log_ticker_on {
                                // Pre-read so the first frame has content without
                                // waiting for the 150 ms tick.
                                log_ticker_line = std::fs::read_to_string(log_path)
                                    .ok()
                                    .and_then(|c| c.lines().last().map(str::to_string))
                                    .unwrap_or_default();
                            }
                            dirty = true;
                            error_set_at = None;
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
                    footer_width = cols;
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

fn pane_env<'a>(
    terminal_id: &'static str,
    port: &'a str,
    session_name: Option<&'a str>,
    role: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut env = vec![("TERMINAL_ID", terminal_id), ("ORCHESTRATION_PORT", port)];
    if let Some(session_name) = session_name.filter(|value| !value.trim().is_empty()) {
        env.push(("CDUO_SESSION_NAME", session_name));
    }
    if let Some(role) = role.filter(|value| !value.trim().is_empty()) {
        env.push(("CDUO_PANE_ROLE", role));
    }
    env
}

struct RuntimeChannels {
    input_tx: mpsc::Sender<(String, String)>,
    control_tx: mpsc::Sender<relay::RelayControl>,
    write_rx: mpsc::Receiver<(String, Vec<u8>)>,
    status_rx: mpsc::Receiver<relay::RelayStatus>,
    hook_ping_rx: mpsc::Receiver<()>,
}

fn drain_paused_writes(
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

fn handle_screen_scroll(
    panes: &mut [Pane; 2],
    pane: PaneId,
    action: GlobalAction,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
) {
    let idx = pane_id_index(pane);
    if panes[idx].agent == "codex" {
        if let Some(bytes) = codex_screen_scroll_bytes(&action) {
            if let Err(err) = panes[idx].write(bytes) {
                *footer_msg = write_error_footer(pane.label(), &err);
                *error_raw_msg = footer_msg.clone();
                *error_set_at = Some(Instant::now());
            }
        }
        return;
    }

    match action {
        GlobalAction::ScrollUp => panes[idx].scroll_up(SCROLL_LINES),
        GlobalAction::ScrollDown => panes[idx].scroll_down(SCROLL_LINES),
        _ => {}
    }
}

fn codex_screen_scroll_bytes(action: &GlobalAction) -> Option<&'static [u8]> {
    match action {
        GlobalAction::ScrollUp => Some(b"\x1b[5~"),
        GlobalAction::ScrollDown => Some(b"\x1b[6~"),
        _ => None,
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

pub(super) fn write_error_footer(target: &str, err: &dyn std::fmt::Display) -> String {
    format!(" relay write failed for pane {target} · {err} ")
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

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
