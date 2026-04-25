use anyhow::Result;
use portable_pty::{CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub struct PtyManager {
    system: Arc<dyn PtySystem>,
}

impl PtyManager {
    pub fn new() -> Result<Self> {
        let system = NativePtySystem::default();
        Ok(Self {
            system: Arc::new(system),
        })
    }

    pub fn spawn(
        &self,
        command: &str,
        args: &[&str],
        cwd: &Path,
        env: &[(&str, &str)],
        cols: u16,
        rows: u16,
    ) -> Result<PtySession> {
        let pty = self.system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let child = pty.slave.spawn_command(cmd)?;
        let pid = child.process_id();
        let master_fd = pty.master.as_raw_fd();

        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
        let (exit_tx, exit_rx) = oneshot::channel::<ExitStatus>();
        let running = Arc::new(AtomicBool::new(true));

        let reader_running = running.clone();
        let reader_master = pty.master.try_clone_reader()?;
        tokio::task::spawn_blocking(move || {
            let mut reader = reader_master;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if tx.blocking_send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
                if !reader_running.load(Ordering::Relaxed) {
                    break;
                }
            }
        });

        let monitor_running = running.clone();
        tokio::task::spawn(async move {
            let mut child = child;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = exit_tx.send(status);
                        break;
                    }
                    Ok(None) => {
                        if !monitor_running.load(Ordering::Relaxed) {
                            break;
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        });

        let writer = pty.master.take_writer()?;

        Ok(PtySession {
            master: pty.master,
            writer,
            pid,
            master_fd,
            rx,
            exit_rx,
            running,
        })
    }
}

#[allow(dead_code)]
pub struct PtySession {
    master: Box<dyn MasterPty>,
    writer: Box<dyn std::io::Write + Send>,
    pid: Option<u32>,
    master_fd: Option<portable_pty::unix::RawFd>,
    rx: mpsc::Receiver<Vec<u8>>,
    exit_rx: oneshot::Receiver<ExitStatus>,
    running: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl PtySession {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn master_fd(&self) -> Option<portable_pty::unix::RawFd> {
        self.master_fd
    }

    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    pub async fn wait_for_exit(&mut self) -> Result<ExitStatus> {
        let rx = self.exit_rx.try_recv().ok();
        if let Some(status) = rx {
            return Ok(status);
        }
        loop {
            if let Ok(status) = self.exit_rx.try_recv() {
                return Ok(status);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }

    pub fn try_wait_for_exit(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.exit_rx.try_recv().ok())
    }

    pub fn close(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_pty_spawn_and_echo() {
        let manager = PtyManager::new().expect("failed to create PtyManager");
        let cwd = std::env::current_dir().expect("failed to get cwd");

        let mut session = manager
            .spawn("cat", &[], &cwd, &[], 80, 24)
            .expect("failed to spawn cat");

        tokio::time::sleep(Duration::from_millis(200)).await;

        session
            .write(b"hello pty\r\n")
            .expect("failed to write to pty");

        let mut output = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), session.read()).await {
                Ok(Some(data)) => {
                    output.extend(data);
                    if output.windows(9).any(|w| w == b"hello pty") {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains("hello pty"),
            "expected echo output, got: {:?}",
            output_str
        );

        session.write(b"\x04").expect("failed to send EOF");

        let exit_status = tokio::time::timeout(Duration::from_secs(5), session.wait_for_exit())
            .await
            .expect("timeout waiting for exit")
            .expect("failed to get exit status");

        assert!(exit_status.success() || exit_status.signal().is_some());
    }

    #[tokio::test]
    async fn test_pty_resize() {
        let manager = PtyManager::new().expect("failed to create PtyManager");
        let cwd = std::env::current_dir().expect("failed to get cwd");

        let mut session = manager
            .spawn("cat", &[], &cwd, &[], 80, 24)
            .expect("failed to spawn cat");

        session.resize(120, 40).expect("failed to resize");

        let size = session.master.get_size().expect("failed to get size");
        assert_eq!(size.rows, 40);
        assert_eq!(size.cols, 120);

        session.close();
    }

    #[tokio::test]
    async fn test_pty_child_exit() {
        let manager = PtyManager::new().expect("failed to create PtyManager");
        let cwd = std::env::current_dir().expect("failed to get cwd");

        let mut session = manager
            .spawn("sh", &["-c", "exit 42"], &cwd, &[], 80, 24)
            .expect("failed to spawn sh");

        let exit_status = tokio::time::timeout(Duration::from_secs(5), session.wait_for_exit())
            .await
            .expect("timeout waiting for exit")
            .expect("failed to get exit status");

        assert_eq!(exit_status.exit_code(), 42);
    }

    #[tokio::test]
    async fn test_pty_env_vars() {
        let manager = PtyManager::new().expect("failed to create PtyManager");
        let cwd = std::env::current_dir().expect("failed to get cwd");

        let mut session = manager
            .spawn(
                "sh",
                &["-c", "echo $TEST_VAR"],
                &cwd,
                &[("TEST_VAR", "pty_works")],
                80,
                24,
            )
            .expect("failed to spawn sh");

        let mut output = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), session.read()).await {
                Ok(Some(data)) => {
                    output.extend(data);
                    if output.windows(9).any(|w| w == b"pty_works") {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains("pty_works"),
            "expected env var output, got: {:?}",
            output_str
        );

        session.close();
    }
}
