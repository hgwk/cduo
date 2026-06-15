use super::*;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[path = "project_tests_part1.rs"]
mod project_tests_part1;
#[path = "project_tests_part2.rs"]
mod project_tests_part2;
