use std::path::Path;

use anyhow::Result;
use ratatui::layout::Size;

use crate::native::access::{agent_args, agent_program, AccessMode};
use crate::native::pane::{Pane, PaneId, PaneSpawnOptions};
use crate::native::runtime::RuntimeOptions;
use crate::native::runtime_loop_support::pane_env;
use crate::native::ui::pane_pty_size;

pub(super) fn spawn_panes(
    opts: &RuntimeOptions,
    cwd: &Path,
    hook_port: u16,
    initial: Size,
) -> Result<[Pane; 2]> {
    let (pane_cols, pane_rows) = pane_pty_size(initial.width, initial.height, opts.split);
    let port_str = hook_port.to_string();
    let mode = AccessMode::from_flags(opts.yolo, opts.full_access)?;

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
    Ok([pane_a, pane_b])
}
