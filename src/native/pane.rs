use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

const SCROLLBACK_LINES: usize = 2_000;
const PTY_OUTPUT_CHANNEL_CAPACITY: usize = 256;

/// Identifies one of the two panes the runtime owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    A,
    B,
}

impl PaneId {
    pub fn label(self) -> &'static str {
        match self {
            PaneId::A => "a",
            PaneId::B => "b",
        }
    }
}

/// A single pane: PTY master + reader thread + vt100 parser.
///
/// The reader thread pushes raw bytes onto an mpsc channel; the runtime drains
/// the channel each frame and feeds the parser. The thread exits naturally
/// when the master PTY is dropped (read returns 0), so we do not retain its
/// JoinHandle.
pub struct Pane {
    pub id: PaneId,
    pub agent: String,
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub writer: Box<dyn Write + Send>,
    pub bytes_rx: mpsc::Receiver<Vec<u8>>,
    pub parser: vt100::Parser,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        agent: &str,
        args: &[&str],
        cwd: &Path,
        cols: u16,
        rows: u16,
        env: &[(&str, &str)],
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .with_context(|| format!("openpty for pane {}", id.label()))?;

        let mut command = CommandBuilder::new(agent);
        for arg in args {
            command.arg(arg);
        }
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for (k, v) in env {
            command.env(*k, *v);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("spawn agent '{agent}' for pane {}", id.label()))?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(PTY_OUTPUT_CHANNEL_CAPACITY);
        thread::Builder::new()
            .name(format!("cduo-pty-reader-{}", id.label()))
            .spawn(move || read_pty_chunks(reader, tx))?;

        Ok(Self {
            id,
            agent: agent.to_string(),
            master: pair.master,
            child,
            writer,
            bytes_rx: rx,
            parser: vt100::Parser::new(rows, cols, SCROLLBACK_LINES),
        })
    }

    /// Pull whatever bytes are queued from the reader thread and feed the
    /// vt100 parser. Returns true if any bytes were processed (caller should
    /// repaint).
    pub fn drain_into_parser(&mut self) -> bool {
        let mut got_data = false;
        loop {
            match self.bytes_rx.try_recv() {
                Ok(chunk) => {
                    self.parser.process(&chunk);
                    got_data = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        got_data
    }

    /// Returns true if the child process has exited.
    pub fn child_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Forward raw bytes to the PTY writer.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the PTY and the parser screen to match.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.set_size(rows, cols);
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_up(&mut self, rows: usize) {
        let next = self.scrollback().saturating_add(rows);
        self.parser.set_scrollback(next);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        let next = self.scrollback().saturating_sub(rows);
        self.parser.set_scrollback(next);
    }

    /// Best-effort kill of the child process.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

fn read_pty_chunks(mut reader: Box<dyn Read + Send>, tx: SyncSender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
fn pty_output_channel_capacity() -> usize {
    PTY_OUTPUT_CHANNEL_CAPACITY
}

/// Runtime focus state. Kept separate so it's trivially testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus(pub PaneId);

impl Focus {
    pub fn next(self) -> Self {
        Focus(match self.0 {
            PaneId::A => PaneId::B,
            PaneId::B => PaneId::A,
        })
    }

    pub fn prev(self) -> Self {
        // Two-pane cycle, so prev == next.
        self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_toggles() {
        let f = Focus(PaneId::A);
        assert_eq!(f.next(), Focus(PaneId::B));
        assert_eq!(f.next().next(), Focus(PaneId::A));
        assert_eq!(f.prev(), Focus(PaneId::B));
    }

    #[test]
    fn pane_id_labels() {
        assert_eq!(PaneId::A.label(), "a");
        assert_eq!(PaneId::B.label(), "b");
    }

    #[test]
    fn pty_output_channel_has_bounded_capacity() {
        assert_eq!(pty_output_channel_capacity(), 256);
    }
}
