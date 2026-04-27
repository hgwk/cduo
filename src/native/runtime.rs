//! Native two-pane runtime: owns Pane A and Pane B PTYs, draws them with
//! ratatui, forwards keys to whichever pane has focus, runs the Claude Stop
//! hook HTTP server, and drives the in-process relay loop. The runtime
//! process is the cduo session — there is no background daemon.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use tokio::sync::{broadcast, mpsc};

use crate::cli::Agent;
use crate::hook::{self, HookEvent};
use crate::native::input::{classify_key, key_to_bytes, GlobalAction};
use crate::native::pane::{Focus, Pane, PaneId};
use crate::native::relay;
use crate::native::ui::{pane_pty_size, ScreenWidget};

const FRAME_BUDGET_MS: u64 = 16;
const POLL_INTERVAL_MS: u64 = 8;
const SCROLL_LINES: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct RuntimeOptions {
    pub agent_a: Agent,
    pub agent_b: Agent,
    pub yolo: bool,
    pub full_access: bool,
    /// Reserved for future "always create a new session" semantics; native
    /// mode currently spawns a fresh session every time so this is a no-op.
    #[allow(dead_code)]
    pub new_session: bool,
}

#[derive(Debug, Clone, Copy)]
enum AccessMode {
    Default,
    Yolo,
    FullAccess,
}

impl AccessMode {
    fn from_flags(yolo: bool, full_access: bool) -> Result<Self> {
        match (yolo, full_access) {
            (true, true) => anyhow::bail!("Use either --yolo or --full-access, not both."),
            (true, false) => Ok(Self::Yolo),
            (false, true) => Ok(Self::FullAccess),
            (false, false) => Ok(Self::Default),
        }
    }
}

fn agent_args(agent: Agent, mode: AccessMode) -> &'static [&'static str] {
    match (agent, mode) {
        (Agent::Codex, AccessMode::Yolo) => &["--dangerously-bypass-approvals-and-sandbox"],
        (Agent::Codex, AccessMode::FullAccess) => &[
            "--sandbox",
            "danger-full-access",
            "--ask-for-approval",
            "never",
        ],
        (Agent::Codex, AccessMode::Default) => &[],
        (Agent::Claude, AccessMode::Yolo) => &["--dangerously-skip-permissions"],
        (Agent::Claude, AccessMode::FullAccess) => &["--permission-mode", "bypassPermissions"],
        (Agent::Claude, AccessMode::Default) => &[],
    }
}

pub async fn run(opts: RuntimeOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("get current dir")?;

    // Validate flags before allocating anything else.
    AccessMode::from_flags(opts.yolo, opts.full_access)?;

    let hook_port = find_available_port(53333).await?;
    let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(64);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let (input_tx, input_rx) = mpsc::channel::<(String, String)>(64);
    let (write_tx, write_rx) = mpsc::channel::<(String, Vec<u8>)>(64);

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
        input_rx,
        write_tx,
        shutdown_rx: shutdown_tx.subscribe(),
    }));

    let join =
        tokio::task::spawn_blocking(move || run_blocking(opts, cwd, hook_port, input_tx, write_rx));
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

fn run_blocking(
    opts: RuntimeOptions,
    cwd: PathBuf,
    hook_port: u16,
    input_tx: mpsc::Sender<(String, String)>,
    write_rx: mpsc::Receiver<(String, Vec<u8>)>,
) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;

    let result = ui_loop(opts, &cwd, hook_port, input_tx, write_rx);

    let mut stdout = io::stdout();
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

fn ui_loop(
    opts: RuntimeOptions,
    cwd: &std::path::Path,
    hook_port: u16,
    input_tx: mpsc::Sender<(String, String)>,
    mut write_rx: mpsc::Receiver<(String, Vec<u8>)>,
) -> Result<()> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let initial = terminal.size()?;
    let (pane_cols, pane_rows) = pane_pty_size(initial.width, initial.height);
    let port_str = hook_port.to_string();
    let mode = AccessMode::from_flags(opts.yolo, opts.full_access)?;

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
        " A:{}  B:{}  · hook:{}  · Ctrl-W: focus  · PageUp/PageDown: scroll  · Ctrl-Q: quit ",
        agent_program(opts.agent_a),
        agent_program(opts.agent_b),
        hook_port,
    );

    // Per-pane buffer that mirrors what we forwarded to the agent. On every
    // \r/\n we flush it as a (pane_id, line) submission for the relay's codex
    // pending-prompt matching.
    let mut input_buf: HashMap<PaneId, Vec<u8>> = HashMap::new();

    'main: loop {
        // Drain any pending relay writes (bracketed-paste bundles + Enter)
        // and forward them to the right pane's PTY.
        loop {
            match write_rx.try_recv() {
                Ok((target, bytes)) => {
                    let idx = match target.as_str() {
                        "a" => 0,
                        "b" => 1,
                        _ => continue,
                    };
                    let _ = panes[idx].write(&bytes);
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
                draw(frame, &panes, focus, &footer_msg);
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
                            dirty = true;
                        }
                        GlobalAction::FocusPrev => {
                            focus = focus.prev();
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
                    let (pane_cols, pane_rows) = pane_pty_size(cols, rows);
                    for pane in panes.iter_mut() {
                        pane.resize(pane_cols, pane_rows);
                    }
                    dirty = true;
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

fn focus_index(focus: Focus) -> usize {
    match focus.0 {
        PaneId::A => 0,
        PaneId::B => 1,
    }
}

fn draw(frame: &mut ratatui::Frame, panes: &[Pane; 2], focus: Focus, footer_msg: &str) {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        frame.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 2);

    let half = body.width / 2;
    let pane_a_area = Rect::new(body.x, body.y, half, body.height);
    let divider_area = Rect::new(body.x + half, body.y, 1, body.height);
    let pane_b_area = Rect::new(
        body.x + half + 1,
        body.y,
        body.width.saturating_sub(half + 1),
        body.height,
    );

    frame.render_widget(
        Paragraph::new(format!(
            " cduo · A:{} | B:{} ",
            panes[0].agent, panes[1].agent
        ))
        .style(Style::default().add_modifier(Modifier::BOLD)),
        header_area,
    );

    render_pane(frame, &panes[0], pane_a_area, focus.0 == PaneId::A);
    render_divider(frame, divider_area);
    render_pane(frame, &panes[1], pane_b_area, focus.0 == PaneId::B);

    frame.render_widget(
        Paragraph::new(footer_msg).style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
}

fn render_pane(frame: &mut ratatui::Frame, pane: &Pane, area: Rect, focused: bool) {
    let scroll = pane.scrollback();
    let title = if scroll > 0 {
        format!(
            " {} {} ↑{} ",
            pane.id.label().to_uppercase(),
            pane.agent,
            scroll
        )
    } else {
        format!(" {} {} ", pane.id.label().to_uppercase(), pane.agent)
    };
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let widget = ScreenWidget {
        screen: pane.parser.screen(),
    };
    frame.render_widget(widget, inner);

    if focused && !pane.parser.screen().hide_cursor() {
        let (cur_row, cur_col) = pane.parser.screen().cursor_position();
        let x = inner.x + cur_col;
        let y = inner.y + cur_row;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            frame.set_cursor_position(ratatui::layout::Position::new(x, y));
        }
    }
}

fn render_divider(frame: &mut ratatui::Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block, area);
}

fn agent_program(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
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

    #[test]
    fn access_mode_rejects_conflicting_flags() {
        assert!(AccessMode::from_flags(true, true).is_err());
    }

    #[test]
    fn access_mode_default() {
        assert!(matches!(
            AccessMode::from_flags(false, false).unwrap(),
            AccessMode::Default
        ));
    }

    #[test]
    fn agent_args_yolo_codex() {
        let args = agent_args(Agent::Codex, AccessMode::Yolo);
        assert_eq!(args, &["--dangerously-bypass-approvals-and-sandbox"]);
    }

    #[test]
    fn agent_args_full_access_codex() {
        let args = agent_args(Agent::Codex, AccessMode::FullAccess);
        assert_eq!(
            args,
            &[
                "--sandbox",
                "danger-full-access",
                "--ask-for-approval",
                "never",
            ]
        );
    }

    #[test]
    fn agent_args_yolo_claude() {
        let args = agent_args(Agent::Claude, AccessMode::Yolo);
        assert_eq!(args, &["--dangerously-skip-permissions"]);
    }

    #[test]
    fn agent_args_full_access_claude() {
        let args = agent_args(Agent::Claude, AccessMode::FullAccess);
        assert_eq!(args, &["--permission-mode", "bypassPermissions"]);
    }

    #[test]
    fn agent_args_default_is_empty() {
        assert!(agent_args(Agent::Claude, AccessMode::Default).is_empty());
        assert!(agent_args(Agent::Codex, AccessMode::Default).is_empty());
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
