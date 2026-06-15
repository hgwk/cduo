use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::cli::SplitLayout;
use crate::native::layout::resize_panes_for_view;
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::relay;
use crate::native::runtime::RuntimeOptions;
use crate::native::runtime_loop::ui_loop;
use crate::native::runtime_status::TrafficCounters;

pub(super) fn run_blocking(
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

pub(super) fn default_footer_message(hook_port: u16, hook_dot: &str) -> String {
    format!(
        " hook:{hook_port}{hook_dot}  · Ctrl-Y: broadcast  · Ctrl-N: names  · Ctrl-W: focus  · Ctrl-P: pause relay  · Ctrl-L: split  · drag: copy  · PageUp/PageDown: scroll  · Ctrl-Q: quit "
    )
}

pub(super) fn empty_traffic_counters() -> TrafficCounters {
    TrafficCounters {
        a_to_b_bytes: 0,
        b_to_a_bytes: 0,
        last_a_to_b_at: None,
        last_b_to_a_at: None,
        samples_a_to_b: std::collections::VecDeque::from(vec![0u64; 8]),
        samples_b_to_a: std::collections::VecDeque::from(vec![0u64; 8]),
        last_sample_at: std::time::Instant::now(),
    }
}

pub(super) fn surface_child_exit(
    panes: &mut [Pane; 2],
    footer_msg: &mut String,
    error_set_at: &mut Option<std::time::Instant>,
    dirty: &mut bool,
) {
    for pane in panes.iter_mut() {
        if pane.child_exited() {
            *footer_msg = format!(
                " pane {} exited · Ctrl-Q to leave ",
                pane.id.label().to_uppercase()
            );
            *error_set_at = None;
            *dirty = true;
        }
    }
}

pub(super) fn sync_maximized_focus(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    panes: &mut [Pane; 2],
    focus: Focus,
    split: SplitLayout,
    maximized: &mut Option<PaneId>,
) -> Result<()> {
    if maximized.is_some() {
        *maximized = Some(focus.0);
        let size = terminal.size()?;
        resize_panes_for_view(panes, size.width, size.height, split, *maximized);
    }
    Ok(())
}

pub(super) fn pane_env<'a>(
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

pub(super) struct RuntimeChannels {
    pub(super) input_tx: mpsc::Sender<(String, String)>,
    pub(super) control_tx: mpsc::Sender<relay::RelayControl>,
    pub(super) write_rx: mpsc::Receiver<(String, Vec<u8>)>,
    pub(super) status_rx: mpsc::Receiver<relay::RelayStatus>,
    pub(super) hook_ping_rx: mpsc::Receiver<()>,
}

pub(super) fn send_control_or_footer(
    control_tx: &mpsc::Sender<relay::RelayControl>,
    control: relay::RelayControl,
    success_footer: impl FnOnce() -> String,
) -> String {
    match control_tx.try_send(control) {
        Ok(()) => success_footer(),
        Err(err) => format!(" relay control unavailable · {err} "),
    }
}

pub(super) fn recent_log_footer(log_path: &std::path::Path) -> String {
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
