use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

use super::TranscriptOutput;

pub fn read_last_assistant(path: &Path) -> TranscriptOutput {
    let Ok(transcript) = std::fs::read_to_string(path) else {
        return TranscriptOutput::empty();
    };

    let Some(output) = transcript
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(assistant_text_from_entry)
    else {
        return TranscriptOutput::empty();
    };

    let mut hasher = Sha256::new();
    hasher.update(output.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    TranscriptOutput::new(output, format!("claude-transcript:{}", &hash_hex[..8]))
}

fn assistant_text_from_entry(entry: Value) -> Option<String> {
    let message = entry.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }

    let content = message.get("content")?;
    let text = match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    part.get("text")
                        .and_then(Value::as_str)
                        .map(str::trim)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_last_assistant_from_transcript() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"first"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"thinking","thinking":"skip"}},{{"type":"text","text":"second"}},{{"type":"text","text":"answer"}}]}}}}"#
        )
        .unwrap();

        let result = read_last_assistant(file.path());
        assert_eq!(result.output, "second\nanswer");
        assert!(result.signature.unwrap().starts_with("claude-transcript:"));
    }
}
