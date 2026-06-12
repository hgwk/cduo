use std::path::Path;

pub fn normalize_prompt_text(value: &str) -> String {
    let mut out = Vec::new();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\u{7f}' | '\u{8}' => {
                out.pop();
            }
            '\x1b' => {
                skip_escape_sequence(&mut chars);
            }
            '\r' => {}
            '\n' => out.push(ch),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }

    out.into_iter().collect::<String>().trim().to_string()
}

fn skip_escape_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        }
        Some(']') => {
            chars.next();
            let mut prev = '\0';
            for ch in chars.by_ref() {
                if ch == '\u{7}' || (prev == '\x1b' && ch == '\\') {
                    break;
                }
                prev = ch;
            }
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

pub fn codex_transcript_contains_user_prompt(path: &Path, expected_prompt: &str) -> bool {
    let expected_prompt = normalize_prompt_text(expected_prompt);
    let compact_expected = compact_prompt_text(&expected_prompt);
    if expected_prompt.is_empty() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|entry| {
            codex_user_text_from_entry(&entry).is_some_and(|text| {
                let compact_text = compact_prompt_text(&text);
                text == expected_prompt
                    || text.contains(&expected_prompt)
                    || expected_prompt.contains(&text)
                    || (!compact_text.is_empty()
                        && !compact_expected.is_empty()
                        && (compact_text.contains(&compact_expected)
                            || compact_expected.contains(&compact_text)))
            })
        })
}

fn compact_prompt_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
}

fn codex_user_text_from_entry(entry: &serde_json::Value) -> Option<String> {
    if entry.get("type").and_then(serde_json::Value::as_str) != Some("response_item") {
        return None;
    }

    let payload = entry.get("payload")?;
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("message")
        || payload.get("role").and_then(serde_json::Value::as_str) != Some("user")
    {
        return None;
    }

    let content = payload.get("content")?;
    let text = match content {
        serde_json::Value::String(text) => normalize_prompt_text(text),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(serde_json::Value::as_str) == Some("input_text") {
                    part.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(normalize_prompt_text)
                        .filter(|text| !text.is_empty())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
