use super::*;
use crossterm::event::KeyEventKind;

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    }
}

#[test]
fn ctrl_q_quits() {
    assert_eq!(
        classify_key(key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        GlobalAction::Quit
    );
}

#[test]
fn ctrl_w_focus_next() {
    assert_eq!(
        classify_key(key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        GlobalAction::FocusNext
    );
}

#[test]
fn ctrl_p_toggles_pause() {
    assert_eq!(
        classify_key(key(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        GlobalAction::TogglePause
    );
}

#[test]
fn ctrl_l_toggles_split() {
    assert_eq!(
        classify_key(key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleSplit
    );
}

#[test]
fn relay_controls_classify() {
    assert_eq!(
        classify_key(key(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        GlobalAction::ManualRelay
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('x'), KeyModifiers::CONTROL)),
        GlobalAction::ClearRelayQueue
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('1'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleRelayAToB
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('2'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleRelayBToA
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('g'), KeyModifiers::CONTROL)),
        GlobalAction::ShowRelayLog
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('z'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleFocusLayout
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        GlobalAction::BroadcastInput
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('n'), KeyModifiers::CONTROL)),
        GlobalAction::EditMetadata
    );
}

#[test]
fn ctrl_t_toggles_log_ticker() {
    assert_eq!(
        classify_key(key(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleLogTicker
    );
    assert_eq!(
        classify_key(key(KeyCode::Char('T'), KeyModifiers::CONTROL)),
        GlobalAction::ToggleLogTicker
    );
}

#[test]
fn ctrl_shift_w_focus_prev() {
    let mods = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    assert_eq!(
        classify_key(key(KeyCode::Char('W'), mods)),
        GlobalAction::FocusPrev
    );
}

#[test]
fn plain_letter_forwards() {
    assert_eq!(
        classify_key(key(KeyCode::Char('a'), KeyModifiers::NONE)),
        GlobalAction::Forward
    );
}

#[test]
fn page_keys_scroll() {
    assert_eq!(
        classify_key(key(KeyCode::PageUp, KeyModifiers::NONE)),
        GlobalAction::ScrollUp
    );
    assert_eq!(
        classify_key(key(KeyCode::PageDown, KeyModifiers::NONE)),
        GlobalAction::ScrollDown
    );
}

#[test]
fn key_to_bytes_ctrl_letters() {
    let bytes = key_to_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
    assert_eq!(bytes, vec![0x03]);
}

#[test]
fn key_to_bytes_arrows() {
    let bytes = key_to_bytes(key(KeyCode::Up, KeyModifiers::NONE)).unwrap();
    assert_eq!(bytes, b"\x1b[A");
}

#[test]
fn key_to_bytes_shift_tab() {
    let bytes = key_to_bytes(key(KeyCode::Tab, KeyModifiers::SHIFT)).unwrap();
    assert_eq!(bytes, b"\x1b[Z");
}

#[test]
fn key_to_bytes_korean_char() {
    let bytes = key_to_bytes(key(KeyCode::Char('하'), KeyModifiers::NONE)).unwrap();
    assert_eq!(bytes, "하".as_bytes());
}
