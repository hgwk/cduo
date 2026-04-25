pub mod claude;
pub mod codex;

use regex::Regex;

fn ansi_regex() -> Regex {
    Regex::new(
        r"(?x)
        \x1b\[[0-9;]*[a-zA-Z]       |  # CSI sequences
        \x1b\][^\x07]*\x07          |  # OSC sequences
        \x1b[>=()]                  |  # Single-char ESC sequences
        \x1b\?[0-9;]+[hl]           |  # DEC private mode
        \[[0-9;?]+[hl]              |  # Bracketed sequences
        \x1b[^\x1b]*\x1b\\          |  # DCS/APC sequences
        \r                          |  # Carriage returns
        [\x00-\x08\x0B-\x1F\x7F-\x9F]  # Control chars
        "
    )
    .unwrap()
}

pub fn strip_ansi(input: &str) -> String {
    ansi_regex().replace_all(input, "").to_string()
}

#[derive(Debug, Clone)]
pub struct ExtractedOutput {
    pub output: String,
    pub signature: Option<String>,
}

impl ExtractedOutput {
    pub fn empty() -> Self {
        Self {
            output: String::new(),
            signature: None,
        }
    }

    pub fn new(output: String, signature: String) -> Self {
        Self {
            output,
            signature: Some(signature),
        }
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.output.is_empty()
    }
}
