use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::native::layout::pane_id_index;
use crate::native::pane::{Pane, PaneId};
use crate::native::runtime_io::{capture_line, write_error_footer};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BroadcastInputAction {
    Editing,
    Cancel,
    Submit(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MetadataInputAction {
    Editing,
    Cancel,
    Submit(String),
}

pub(super) fn handle_broadcast_key(key: KeyEvent, buffer: &mut String) -> BroadcastInputAction {
    match key.code {
        KeyCode::Esc => BroadcastInputAction::Cancel,
        KeyCode::Char('y') | KeyCode::Char('Y')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            BroadcastInputAction::Cancel
        }
        KeyCode::Enter => {
            let prompt = buffer.trim().to_string();
            if prompt.is_empty() {
                BroadcastInputAction::Cancel
            } else {
                BroadcastInputAction::Submit(prompt)
            }
        }
        KeyCode::Backspace => {
            buffer.pop();
            BroadcastInputAction::Editing
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            buffer.push(c);
            BroadcastInputAction::Editing
        }
        _ => BroadcastInputAction::Editing,
    }
}

pub(super) fn handle_metadata_key(key: KeyEvent, buffer: &mut String) -> MetadataInputAction {
    match key.code {
        KeyCode::Esc => MetadataInputAction::Cancel,
        KeyCode::Char('n') | KeyCode::Char('N')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            MetadataInputAction::Cancel
        }
        KeyCode::Enter => {
            let input = buffer.trim().to_string();
            if input.is_empty() {
                MetadataInputAction::Cancel
            } else {
                MetadataInputAction::Submit(input)
            }
        }
        KeyCode::Backspace => {
            buffer.pop();
            MetadataInputAction::Editing
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            buffer.push(c);
            MetadataInputAction::Editing
        }
        _ => MetadataInputAction::Editing,
    }
}

pub(super) fn broadcast_prompt_bytes(prompt: &str) -> Vec<u8> {
    format!("User says: {prompt}\r").into_bytes()
}

pub(super) fn send_broadcast_prompt(
    panes: &mut [Pane; 2],
    prompt: &str,
    input_buf: &mut HashMap<PaneId, Vec<u8>>,
    input_tx: &mpsc::Sender<(String, String)>,
    footer_msg: &mut String,
    error_set_at: &mut Option<Instant>,
    error_raw_msg: &mut String,
) {
    let bytes = broadcast_prompt_bytes(prompt);
    let mut sent = 0;
    for pane in [PaneId::A, PaneId::B] {
        let idx = pane_id_index(pane);
        match panes[idx].write(&bytes) {
            Ok(()) => {
                capture_line(pane, &bytes, input_buf, input_tx);
                sent += 1;
            }
            Err(err) => {
                *footer_msg = write_error_footer(pane.label(), &err);
                *error_raw_msg = footer_msg.clone();
                *error_set_at = Some(Instant::now());
                return;
            }
        }
    }

    *footer_msg = format!(" broadcast sent to {sent} panes · relay paused · Ctrl-P: resume relay ");
    *error_set_at = None;
}

pub(super) fn broadcast_input_footer(buffer: &str, elapsed: Duration) -> String {
    let caret = crate::native::footer::broadcast_caret_glyph(elapsed);
    format!(" broadcast> {buffer}{caret} · Enter: send · Esc: cancel ")
}

pub(super) fn metadata_input_footer(buffer: &str) -> String {
    format!(" metadata> {buffer} · Enter: apply · Esc: cancel ")
}

pub(super) fn current_metadata_input(panes: &[Pane; 2]) -> String {
    metadata_input_value(
        panes[0].session_name.as_deref(),
        panes[0].role.as_deref(),
        panes[1].role.as_deref(),
    )
}

pub(super) fn metadata_input_value(
    session_name: Option<&str>,
    role_a: Option<&str>,
    role_b: Option<&str>,
) -> String {
    let session = format_metadata_value(session_name);
    let role_a = format_metadata_value(role_a);
    let role_b = format_metadata_value(role_b);
    format!("session={session} a={role_a} b={role_b}")
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MetadataUpdate {
    pub(super) session_name: Option<Option<String>>,
    pub(super) role_a: Option<Option<String>>,
    pub(super) role_b: Option<Option<String>>,
}

pub(super) fn parse_metadata_update(input: &str) -> Result<MetadataUpdate, String> {
    let mut update = MetadataUpdate::default();
    for token in split_metadata_tokens(input)? {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!("expected key=value, got '{token}'"));
        };
        let value = metadata_value(value);
        match key {
            "session" | "name" | "session-name" | "session_name" => {
                update.session_name = Some(value);
            }
            "a" | "role-a" | "role_a" => {
                update.role_a = Some(value);
            }
            "b" | "role-b" | "role_b" => {
                update.role_b = Some(value);
            }
            _ => return Err(format!("unknown key '{key}'")),
        }
    }
    if update == MetadataUpdate::default() {
        return Err("no metadata fields provided".to_string());
    }
    Ok(update)
}

pub(super) fn split_metadata_tokens(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
            }
            _ => current.push(ch),
        }
    }
    if escaped {
        current.push('\\');
    }
    if in_quotes {
        return Err("unterminated quoted metadata value".to_string());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

pub(super) fn metadata_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == "-" || value.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(value.to_string())
    }
}

pub(super) fn format_metadata_value(value: Option<&str>) -> String {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return "-".to_string();
    };
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\' | '='))
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

pub(super) fn apply_metadata_update(panes: &mut [Pane; 2], update: MetadataUpdate) -> String {
    if let Some(session_name) = update.session_name {
        for pane in panes.iter_mut() {
            pane.session_name = session_name.clone();
        }
    }
    if let Some(role) = update.role_a {
        panes[0].role = role;
    }
    if let Some(role) = update.role_b {
        panes[1].role = role;
    }
    format!(" metadata updated · {} ", current_metadata_input(panes))
}
