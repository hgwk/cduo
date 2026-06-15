//! Native split-pane TUI runtime backed by ratatui + vt100.
//!
//! The runtime owns two PTYs (pane A and pane B), drives one ratatui draw loop,
//! and runs the existing transcript-based relay logic in-process. There is no
//! daemon process or attach socket on this path; the running cduo binary IS the
//! session.

pub mod access;
pub(crate) mod footer;
pub mod input;
pub mod layout;
pub mod pane;
pub mod relay;
pub(super) mod relay_control;
pub(super) mod relay_delivery;
pub(super) mod relay_handlers;
pub mod render;
pub mod runtime;
pub(super) mod runtime_events;
pub(super) mod runtime_io;
pub(super) mod runtime_loop;
pub(super) mod runtime_loop_spawn;
pub(super) mod runtime_loop_support;
pub(super) mod runtime_metadata;
pub(super) mod runtime_mouse_events;
pub(super) mod runtime_status;
pub mod selection;
pub mod ui;
