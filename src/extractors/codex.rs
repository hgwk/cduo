use regex::Regex;

use super::{strip_ansi, ExtractedOutput};

fn status_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"(?i)^Working(?:\b|$)").unwrap(),
        Regex::new(r"(?i)^Starting ").unwrap(),
        Regex::new(r"(?i)^MCP startup incomplete").unwrap(),
        Regex::new(r"(?i)^The .* MCP server .*$").unwrap(),
        Regex::new(r"^export [A-Z0-9_]+=.+").unwrap(),
        Regex::new(r"(?i)^Token usage:").unwrap(),
        Regex::new(r"(?i)^Tip:").unwrap(),
        Regex::new(r"^⚠\s+").unwrap(),
        Regex::new(r"^[✻✳✶✢✦·✽]\s+").unwrap(),
        Regex::new(r"(?i)^send q or ctrl\+c to exit\b").unwrap(),
        Regex::new(r"(?i)^press enter to send\b").unwrap(),
        Regex::new(r"(?i)^Claude is still responding$").unwrap(),
    ]
}

fn meta_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"(?i)^gpt-[\w.-]+").unwrap(),
        Regex::new(r"^o\d").unwrap(),
        Regex::new(r"\bContext \[").unwrap(),
        Regex::new(r"weekly \d+%").unwrap(),
        Regex::new(r"(?i)esc to interrupt").unwrap(),
    ]
}

fn prompt_regex() -> Regex {
    Regex::new(r"^›(?:\s.*)?$").unwrap()
}

fn dash_line_regex() -> Regex {
    Regex::new(r"^─+$").unwrap()
}

fn codex_prompt_regex() -> Regex {
    Regex::new(r"^❯\s*$").unwrap()
}

fn shell_prompt_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"^[#$%]\s+").unwrap(),
        Regex::new(r"^[^ \t\r\n]+@[^ \t\r\n]+(?::[^ \t\r\n]+)?[$#]\s+").unwrap(),
    ]
}

fn role_line_regex() -> Regex {
    Regex::new(r"^(?i)(assistant|codex|user)$").unwrap()
}

fn leading_bullet_regex() -> Regex {
    Regex::new(r"^[•◦]\s+").unwrap()
}

fn is_codex_status_line(line: &str) -> bool {
    status_patterns().iter().any(|p| p.is_match(line))
}

fn is_codex_meta_line(line: &str) -> bool {
    meta_patterns().iter().any(|p| p.is_match(line))
}

fn is_codex_prompt_line(line: &str) -> bool {
    prompt_regex().is_match(line) || dash_line_regex().is_match(line) || codex_prompt_regex().is_match(line)
}

fn is_codex_role_line(line: &str) -> bool {
    role_line_regex().is_match(line)
}

fn is_shell_prompt_line(line: &str) -> bool {
    shell_prompt_patterns().iter().any(|p| p.is_match(line))
}

fn normalize_codex_line(line: &str) -> String {
    leading_bullet_regex().replace(line, "").trim().to_string()
}

fn is_within_leading_status_block(lines: &[String], index: usize) -> bool {
    let mut start = index as isize;

    while start >= 0 && !normalize_codex_line(&lines[start as usize]).is_empty() {
        start -= 1;
    }

    start += 1;
    while (start as usize) < lines.len() && normalize_codex_line(&lines[start as usize]).is_empty() {
        start += 1;
    }

    if (start as usize) > index {
        return false;
    }

    is_codex_status_line(&normalize_codex_line(&lines[start as usize]))
}

fn clean_codex_lines(block: &str) -> String {
    let lines: Vec<String> = block.split('\n').map(|s| s.to_string()).collect();
    let mut normalized: Vec<String> = Vec::new();
    let mut skip_status_block = false;

    for line in &lines {
        let trimmed = normalize_codex_line(line);
        if trimmed.is_empty() {
            if skip_status_block {
                skip_status_block = false;
            }
            continue;
        }

        if skip_status_block {
            continue;
        }

        if is_shell_prompt_line(&trimmed) {
            if !normalized.is_empty() {
                break;
            }
            continue;
        }

        if is_codex_prompt_line(&trimmed) || is_codex_meta_line(&trimmed) {
            if !normalized.is_empty() {
                break;
            }
            continue;
        }

        if is_codex_status_line(&trimmed) {
            if !normalized.is_empty() {
                break;
            }
            skip_status_block = true;
            continue;
        }

        if is_codex_role_line(&trimmed) {
            if trimmed.to_lowercase() == "user" {
                return String::new();
            }

            if !normalized.is_empty() {
                break;
            }
            continue;
        }

        normalized.push(trimmed);
    }

    collapse_whitespace(&normalized.join(" "))
}

fn trim_after_last_codex_prompt(text: &str) -> &str {
    let prompt_re = Regex::new(r"(?m)^›(?:\s.*)?$").unwrap();
    let mut last_boundary = None;

    for m in prompt_re.find_iter(text) {
        last_boundary = Some(m.start());
    }

    match last_boundary {
        Some(idx) => &text[..idx],
        None => text,
    }
}

fn extract_codex_block_output(text: &str) -> Option<ExtractedOutput> {
    let mut last_valid = None;

    let blocks: Vec<&str> = text.split("\n\n").collect();
    let mut offset = 0;

    for block in &blocks {
        let candidate = clean_codex_lines(block);
        if !candidate.is_empty() && !is_codex_status_line(&candidate) {
            last_valid = Some(ExtractedOutput::new(
                candidate.clone(),
                format!("codex:block:{offset}:{}", candidate.len()),
            ));
        }
        offset += block.len() + 2;
    }

    last_valid
}

fn extract_codex_tail_output(text: &str) -> Option<ExtractedOutput> {
    let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    let mut collected: Vec<String> = Vec::new();
    let mut start_line_index: Option<usize> = None;

    for index in (0..lines.len()).rev() {
        let trimmed = normalize_codex_line(&lines[index]);

        if trimmed.is_empty() {
            if !collected.is_empty() {
                break;
            }
            continue;
        }

        if is_within_leading_status_block(&lines, index) {
            if !collected.is_empty() {
                break;
            }
            continue;
        }

        if is_shell_prompt_line(&trimmed) {
            if !collected.is_empty() {
                break;
            }
            continue;
        }

        if is_codex_prompt_line(&trimmed) || is_codex_status_line(&trimmed) || is_codex_meta_line(&trimmed) {
            if !collected.is_empty() {
                break;
            }
            continue;
        }

        if is_codex_role_line(&trimmed) {
            if trimmed.to_lowercase() == "user" {
                return None;
            }

            if !collected.is_empty() {
                break;
            }
            continue;
        }

        collected.insert(0, lines[index].clone());
        start_line_index = Some(index);
    }

    let candidate = clean_codex_lines(&collected.join("\n"));
    if candidate.is_empty() || is_codex_status_line(&candidate) {
        return None;
    }

    let sig = format!("codex:tail:{}:{}", start_line_index.unwrap_or(0), candidate.len());
    Some(ExtractedOutput::new(candidate, sig))
}

pub fn extract(buffer: &str) -> ExtractedOutput {
    let cleaned = strip_ansi(buffer);
    let text = trim_after_last_codex_prompt(&cleaned);

    extract_codex_block_output(text)
        .or_else(|| extract_codex_tail_output(text))
        .unwrap_or_else(ExtractedOutput::empty)
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
    fn extracts_simple_codex_output() {
        let result = extract("% codex --yolo\nassistant\nImplemented auth API\n\n› ");
        assert_eq!(result.output, "Implemented auth API");
    }

    #[test]
    fn ignores_user_role_blocks() {
        let result = extract("user\nplease review this\n\n› ");
        assert!(result.output.is_empty());
    }

    #[test]
    fn ignores_mcp_warning_blocks() {
        let result = extract(
            "⚠ MCP startup incomplete (failed:\n  stripe)\n\n› Run /review on my current changes\n\n  gpt-5.4 xhigh · 001- · Context [    …]",
        );
        assert!(result.output.is_empty());
    }

    #[test]
    fn ignores_startup_bootstrap_blocks() {
        let result = extract(
            "export TERMINAL_ID=a ORCHESTRATION_PORT=\n53334 TERM=xterm-256color\ncodex\ncduo codex\n\n› Improve documentation in @filename\n\n  gpt-5.4 xhigh · 001- · Context [    …]",
        );
        assert!(result.output.is_empty());
    }

    #[test]
    fn strips_ansi_before_processing() {
        let result = extract("\x1b[32mImplemented auth API\x1b[0m\n\n› ");
        assert_eq!(result.output, "Implemented auth API");
    }

    #[test]
    fn trims_after_last_prompt() {
        let result = extract("first answer\n\nsecond answer\n› ");
        assert_eq!(result.output, "second answer");
    }

    #[test]
    fn filters_status_lines() {
        let result = extract("Working on it\n\nReal answer here\n\n› ");
        assert_eq!(result.output, "Real answer here");
    }

    #[test]
    fn filters_meta_lines() {
        let result = extract("gpt-4o-high\nReal answer\n\n› ");
        assert_eq!(result.output, "Real answer");
    }

    #[test]
    fn filters_token_usage() {
        let result = extract("Token usage: 1234 tokens\n\nReal answer\n\n› ");
        assert_eq!(result.output, "Real answer");
    }

    #[test]
    fn signature_format() {
        let result = extract("assistant\nhello world\n\n› ");
        assert!(result.signature.is_some());
        let sig = result.signature.unwrap();
        assert!(sig.starts_with("codex:"));
    }

    #[test]
    fn handles_dash_prompt() {
        let result = extract("answer here\n──────────\n");
        assert_eq!(result.output, "answer here");
    }

    #[test]
    fn handles_codex_role_line() {
        let result = extract("codex\nImplemented feature\n\n› ");
        assert_eq!(result.output, "Implemented feature");
    }

    #[test]
    fn tail_extraction_fallback() {
        let result = extract("Working...\n\nActual response line\n› ");
        assert_eq!(result.output, "Actual response line");
    }

    #[test]
    fn filters_shell_prompts() {
        let result = extract("$ some command\n\nReal output\n\n› ");
        assert_eq!(result.output, "Real output");
    }

    #[test]
    fn filters_export_lines() {
        let result = extract("export FOO=bar\n\nReal content\n\n› ");
        assert_eq!(result.output, "Real content");
    }

    #[test]
    fn filters_tip_lines() {
        let result = extract("Tip: use /help\n\nReal answer\n\n› ");
        assert_eq!(result.output, "Real answer");
    }

    #[test]
    fn filters_warning_lines() {
        let result = extract("⚠ something went wrong\n\nReal answer\n\n› ");
        assert_eq!(result.output, "Real answer");
    }
}
