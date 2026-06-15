use super::*;

fn make_app() -> (Router, mpsc::Receiver<HookEvent>) {
    let (tx, rx) = mpsc::channel::<HookEvent>(16);
    let state = HookState {
        relay_tx: tx,
        ping_tx: None,
        expected_pair_id: None,
    };
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(state);
    (app, rx)
}

#[path = "hook_tests_part1.rs"]
mod hook_tests_part1;
#[path = "hook_tests_part2.rs"]
mod hook_tests_part2;
#[path = "hook_tests_part3.rs"]
mod hook_tests_part3;
