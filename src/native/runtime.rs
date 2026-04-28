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

use tokio::sync::{broadcast, mpsc};

use crate::cli::{Agent, SplitLayout};
use crate::hook::{self, HookEvent};
use crate::native::access::{agent_args, agent_program, AccessMode};
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

    let hook_port = find_available_port(preferred_hook_port()).await?;
    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(64);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(64);
    let (write_tx, write_rx) = mpsc::channel::<(String, Vec<u8>)>(64);
    let (control_tx, control_rx) = mpsc::channel::<relay::RelayControl>(64);
    if let Ok(prefix) = std::env::var("CDUO_RELAY_PREFIX") {
        if !prefix.is_empty() {
            let _ = control_tx.try_send(relay::RelayControl::SetPrefix(Some(prefix)));
        }
    }

    tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            hook::run_hook_server(hook_port, shutdown_rx, hook_tx).await;
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
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    let join = tokio::task::spawn_blocking(move || {
        run_blocking(
            opts, cwd, hook_port, log_path, input_tx, control_tx, write_rx,
        )
    });
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

async fn find_available_port(start: u16) -> Result<u16> {
    for port in start..start + 100 {
        if tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(port);
        }
    }
    anyhow::bail!("No available port found in range {start}-{}", start + 99)
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
    input_tx: mpsc::Sender<(String, String)>,
    control_tx: mpsc::Sender<relay::RelayControl>,
    write_rx: mpsc::Receiver<(String, Vec<u8>)>,
) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).context("enter alt screen")?;

    let result = ui_loop(
        opts, &cwd, hook_port, &log_path, input_tx, control_tx, write_rx,
    );

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
    input_tx: mpsc::Sender<(String, String)>,
    control_tx: mpsc::Sender<relay::RelayControl>,
    mut write_rx: mpsc::Receiver<(String, Vec<u8>)>,
) -> Result<()> {
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
    let mut dirty = true;
    let mut footer_msg = format!(
        " A:{}  B:{}  · hook:{}  · Ctrl-W: focus  · Ctrl-P: pause relay  · Ctrl-L: split  · drag: copy pane text  · PageUp/PageDown: scroll  · Ctrl-Q: quit ",
        agent_program(opts.agent_a),
        agent_program(opts.agent_b),
        hook_port,
    );
    let default_footer_msg = footer_msg.clone();
    let mut selection: Option<MouseSelection> = None;
    let mut relay_paused = false;
    let mut paused_writes: VecDeque<(String, Vec<u8>)> = VecDeque::new();
    let mut a_to_b_enabled = true;
    let mut b_to_a_enabled = true;
    let mut maximized: Option<PaneId> = None;

    // Per-pane buffer that mirrors what we forwarded to the agent. On every
    // \r/\n we flush it as a (pane_id, line) submission for the relay's codex
    // pending-prompt matching.
    let mut input_buf: HashMap<PaneId, Vec<u8>> = HashMap::new();

    'main: loop {
        if !relay_paused {
            while let Some((target, bytes)) = paused_writes.pop_front() {
                write_to_target(&mut panes, &target, &bytes);
            }
        }

        // Drain any pending relay writes (bracketed-paste bundles + Enter)
        // and forward them to the right pane's PTY.
        loop {
            match write_rx.try_recv() {
                Ok((target, bytes)) => {
                    if relay_paused {
                        paused_writes.push_back((target, bytes));
                        footer_msg = pause_footer(paused_writes.len());
                        dirty = true;
                    } else {
                        write_to_target(&mut panes, &target, &bytes);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
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

        if dirty && last_frame.elapsed() >= Duration::from_millis(FRAME_BUDGET_MS) {
            terminal.draw(|frame| {
                draw(
                    frame,
                    &panes,
                    focus,
                    &footer_msg,
                    selection,
                    split,
                    maximized,
                );
            })?;
            last_frame = Instant::now();
            dirty = false;
        }

        if event::poll(Duration::from_millis(POLL_INTERVAL_MS))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
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
                            dirty = true;
                        }
                        GlobalAction::ManualRelay => {
                            let pane_id = focus.0.label().to_string();
                            let _ = control_tx.try_send(relay::RelayControl::ManualRelay {
                                pane_id: pane_id.clone(),
                            });
                            footer_msg = format!(
                                " manual relay requested from pane {} ",
                                pane_id.to_uppercase()
                            );
                            dirty = true;
                        }
                        GlobalAction::ClearRelayQueue => {
                            let cleared = paused_writes.len();
                            paused_writes.clear();
                            footer_msg =
                                format!(" relay queue cleared · dropped writes: {cleared} ");
                            dirty = true;
                        }
                        GlobalAction::ToggleRelayAToB => {
                            a_to_b_enabled = !a_to_b_enabled;
                            let _ = control_tx.try_send(relay::RelayControl::SetRoute {
                                source: "a".to_string(),
                                target: "b".to_string(),
                                enabled: a_to_b_enabled,
                            });
                            footer_msg = route_footer("A→B", a_to_b_enabled);
                            dirty = true;
                        }
                        GlobalAction::ToggleRelayBToA => {
                            b_to_a_enabled = !b_to_a_enabled;
                            let _ = control_tx.try_send(relay::RelayControl::SetRoute {
                                source: "b".to_string(),
                                target: "a".to_string(),
                                enabled: b_to_a_enabled,
                            });
                            footer_msg = route_footer("B→A", b_to_a_enabled);
                            dirty = true;
                        }
                        GlobalAction::ShowRelayLog => {
                            footer_msg = recent_log_footer(log_path);
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
                                let _ = panes[idx].write(&bytes);
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
                                }
                                dirty = true;
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            let pane =
                                mouse_pane(mouse.column, mouse.row, layouts).unwrap_or(focus.0);
                            panes[pane_id_index(pane)].scroll_up(SCROLL_LINES);
                            dirty = true;
                        }
                        MouseEventKind::ScrollDown => {
                            let pane =
                                mouse_pane(mouse.column, mouse.row, layouts).unwrap_or(focus.0);
                            panes[pane_id_index(pane)].scroll_down(SCROLL_LINES);
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
                dirty = true;
            }
        }
    }

    for pane in panes.iter_mut() {
        pane.kill();
    }
    Ok(())
}

fn write_to_target(panes: &mut [Pane; 2], target: &str, bytes: &[u8]) {
    let idx = match target {
        "a" => 0,
        "b" => 1,
        _ => return,
    };
    let _ = panes[idx].write(bytes);
}

fn pause_footer(queued_writes: usize) -> String {
    format!(" relay paused · queued writes: {queued_writes} · Ctrl-P: resume ")
}

fn route_footer(route: &str, enabled: bool) -> String {
    let state = if enabled { "on" } else { "off" };
    format!(" relay {route}: {state} · Ctrl-1: A→B · Ctrl-2: B→A ")
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
}
