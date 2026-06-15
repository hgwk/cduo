//! Native two-pane runtime: owns Pane A and Pane B PTYs, draws them with
//! ratatui, forwards keys to whichever pane has focus, runs the Claude Stop
//! hook HTTP server, and drives the in-process relay loop. The runtime
//! process is the cduo session — there is no background daemon.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::cli::{Agent, SplitLayout};
use crate::hook::{self, HookEvent};
use crate::native::access::{agent_program, AccessMode};
use crate::native::relay;

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

    let channels = crate::native::runtime_loop_support::RuntimeChannels {
        input_tx,
        control_tx,
        write_rx,
        status_rx,
        hook_ping_rx,
    };
    let join = tokio::task::spawn_blocking(move || {
        crate::native::runtime_loop_support::run_blocking(opts, cwd, hook_port, log_path, channels)
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

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
