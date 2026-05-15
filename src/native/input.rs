use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a crossterm `KeyEvent` into the byte sequence a typical xterm-style
/// PTY child expects on stdin. Returns `None` for keys we don't translate.
pub fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let mut out: Vec<u8> = Vec::with_capacity(8);
    if alt {
        out.push(0x1b);
    }

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let upper = c.to_ascii_uppercase();
                let byte = match upper {
                    '@'..='_' => (upper as u8) & 0x1f,
                    '?' => 0x7f,
                    ' ' => 0x00,
                    _ => return None,
                };
                out.push(byte);
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                out.extend_from_slice(s.as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Tab => {
            if shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                out.push(b'\t');
            }
        }
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(n) => {
            let seq: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => return None,
            };
            out.extend_from_slice(seq);
        }
        _ => return None,
    }

    Some(out)
}

/// Top-level intent the runtime derives from a key press *before* forwarding
/// to the focused pane.
#[derive(Debug, PartialEq, Eq)]
pub enum GlobalAction {
    /// No global handling; forward bytes to focused pane.
    Forward,
    /// Quit the runtime cleanly.
    Quit,
    /// Move focus to the next pane.
    FocusNext,
    /// Move focus to the previous pane.
    FocusPrev,
    /// Pause or resume automatic relay delivery.
    TogglePause,
    /// Toggle the pane split layout between columns and rows.
    ToggleSplit,
    /// Manually relay the latest answer from the focused pane.
    ManualRelay,
    /// Clear queued relay writes while relay delivery is paused.
    ClearRelayQueue,
    /// Toggle automatic relay from pane A to pane B.
    ToggleRelayAToB,
    /// Toggle automatic relay from pane B to pane A.
    ToggleRelayBToA,
    /// Show recent relay activity in the footer.
    ShowRelayLog,
    /// Toggle focused pane maximize layout preset.
    ToggleFocusLayout,
    /// Open a footer prompt that sends one input to both panes.
    BroadcastInput,
    /// Scroll the focused pane upward through scrollback.
    ScrollUp,
    /// Scroll the focused pane downward toward the live screen.
    ScrollDown,
    /// Toggle the scrolling log ticker in the footer.
    ToggleLogTicker,
}

/// Classify a key press at the runtime level. Ctrl-Q quits, Ctrl-W cycles
/// focus forward, Ctrl-W with Shift cycles backward, Ctrl-P toggles relay
/// pause, Ctrl-L toggles split layout, Ctrl-R triggers manual relay, Ctrl-X
/// clears queued relay writes, Ctrl-1/Ctrl-2 toggle relay directions, Ctrl-G
/// shows recent relay activity, Ctrl-Z toggles focused-pane maximize,
/// Ctrl-Y opens broadcast input for both panes, and Ctrl-T toggles the
/// scrolling log ticker in the footer.
/// Everything else is forwarded.
pub fn classify_key(key: KeyEvent) -> GlobalAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') if ctrl => GlobalAction::Quit,
        KeyCode::Char('w') | KeyCode::Char('W') if ctrl => {
            if shift {
                GlobalAction::FocusPrev
            } else {
                GlobalAction::FocusNext
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') if ctrl => GlobalAction::TogglePause,
        KeyCode::Char('l') | KeyCode::Char('L') if ctrl => GlobalAction::ToggleSplit,
        KeyCode::Char('r') | KeyCode::Char('R') if ctrl => GlobalAction::ManualRelay,
        KeyCode::Char('x') | KeyCode::Char('X') if ctrl => GlobalAction::ClearRelayQueue,
        KeyCode::Char('1') if ctrl => GlobalAction::ToggleRelayAToB,
        KeyCode::Char('2') if ctrl => GlobalAction::ToggleRelayBToA,
        KeyCode::Char('g') | KeyCode::Char('G') if ctrl => GlobalAction::ShowRelayLog,
        KeyCode::Char('z') | KeyCode::Char('Z') if ctrl => GlobalAction::ToggleFocusLayout,
        KeyCode::Char('y') | KeyCode::Char('Y') if ctrl => GlobalAction::BroadcastInput,
        KeyCode::Char('t') | KeyCode::Char('T') if ctrl => GlobalAction::ToggleLogTicker,
        KeyCode::PageUp => GlobalAction::ScrollUp,
        KeyCode::PageDown => GlobalAction::ScrollDown,
        _ => GlobalAction::Forward,
    }
}

#[cfg(test)]
mod tests {
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
}
