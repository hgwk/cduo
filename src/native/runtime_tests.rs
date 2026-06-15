use super::*;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

use crate::native::input::GlobalAction;
use crate::native::pane::PaneId;
use crate::native::runtime_io::*;
use crate::native::runtime_loop_support::*;
use crate::native::runtime_metadata::*;
use crate::native::runtime_status::*;

include!("runtime_tests_part1.rs");
include!("runtime_tests_part2.rs");
include!("runtime_tests_part3.rs");
