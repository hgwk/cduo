use super::*;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

use crate::native::pane::PaneId;
use crate::native::runtime_io::*;
use crate::native::runtime_loop_support::*;
use crate::native::runtime_metadata::*;
use crate::native::runtime_status::*;

fn test_traffic() -> TrafficCounters {
    let now = Instant::now();
    TrafficCounters {
        a_to_b_bytes: 0,
        b_to_a_bytes: 0,
        last_a_to_b_at: None,
        last_b_to_a_at: None,
        samples_a_to_b: VecDeque::from(vec![1, 2, 3]),
        samples_b_to_a: VecDeque::from(vec![4, 5, 6]),
        last_sample_at: now,
    }
}

#[path = "runtime_tests_part1.rs"]
mod runtime_tests_part1;
#[path = "runtime_tests_part2.rs"]
mod runtime_tests_part2;
#[path = "runtime_tests_part3.rs"]
mod runtime_tests_part3;
