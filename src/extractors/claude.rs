use regex::Regex;

use super::{strip_ansi, ExtractedOutput};

fn stop_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"[✻✳✶✢✦·✽]").unwrap(),
        Regex::new(r"(?m)^>").unwrap(),
        Regex::new(r"(?m)^─+$").unwrap(),
    ]
}

fn noise_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"\(esc to interrupt\)").unwrap(),
        Regex::new(r"\? for shortcuts").unwrap(),
        Regex::new(r"Thinking (on|off)").unwrap(),
        Regex::new(r"ctrl-r to search").unwrap(),
        Regex::new(r"toggle\)").unwrap(),
    ]
}

pub fn extract(buffer: &str) -> ExtractedOutput {
    let text = strip_ansi(buffer);
    let last_record_index = match text.rfind('⏺') {
        Some(idx) => idx,
        None => return ExtractedOutput::empty(),
    };

    let marker_len = '⏺'.len_utf8();
    let mut response = text[last_record_index + marker_len..].to_string();

    for pattern in stop_patterns() {
        if let Some(m) = pattern.find(&response) {
            response.truncate(m.start());
            break;
        }
    }

    for pattern in noise_patterns() {
        response = pattern.replace_all(&response, "").to_string();
    }

    let response = collapse_whitespace(&response);

    if response.is_empty() {
        ExtractedOutput::empty()
    } else {
        let signature = format!("claude:{last_record_index}:{}", response.len());
        ExtractedOutput::new(response, signature)
    }
}

fn collapse_whitespace(s: &str) -> String {
    Regex::new(r"\s+")
        .unwrap()
        .replace_all(s, " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_completion() {
        let result = extract("noise\n⏺ Implemented the fix\n");
        assert_eq!(result.output, "Implemented the fix");
        assert!(result.signature.is_some());
    }

    #[test]
    fn extracts_with_ansi_noise() {
        let result = extract("\x1b[32m⏺\x1b[0m Done\x1b[0m");
        assert_eq!(result.output, "Done");
    }

    #[test]
    fn stops_at_star_pattern() {
        let result = extract("⏺ Here is the answer ✻ more stuff");
        assert_eq!(result.output, "Here is the answer");
    }

    #[test]
    fn removes_noise_patterns() {
        let result = extract("⏺ (esc to interrupt) hello ? for shortcuts world");
        assert_eq!(result.output, "hello world");
    }

    #[test]
    fn returns_empty_without_marker() {
        let result = extract("no marker here");
        assert!(result.output.is_empty());
        assert!(result.signature.is_none());
    }

    #[test]
    fn uses_last_marker() {
        let result = extract("⏺ first ⏺ second answer");
        assert_eq!(result.output, "second answer");
    }

    #[test]
    fn stops_at_dash_line() {
        let result = extract("⏺ real answer\n──────────\nfooter");
        assert_eq!(result.output, "real answer");
    }

    #[test]
    fn stops_at_greater_than() {
        let result = extract("⏺ answer\n> prompt line");
        assert_eq!(result.output, "answer");
    }

    #[test]
    fn signature_format() {
        let result = extract("prefix⏺ hello");
        assert!(result.signature.is_some());
        let sig = result.signature.unwrap();
        assert!(sig.starts_with("claude:"));
    }

    #[test]
    fn collapses_multiple_whitespace() {
        let result = extract("⏺ hello    world\n\n  test");
        assert_eq!(result.output, "hello world test");
    }
}
