//! Native split-pane TUI runtime backed by ratatui + vt100.
//!
//! The runtime owns two PTYs (pane A and pane B), drives one ratatui draw loop,
//! and runs the existing transcript-based relay logic in-process. There is no
//! daemon process or attach socket on this path; the running cduo binary IS the
//! session.

pub mod input;
pub mod pane;
pub mod relay;
pub mod runtime;
pub mod ui;
