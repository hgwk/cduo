use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

use super::TranscriptOutput;

fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    result[..4].iter().map(|b| format!("{:02x}", b)).collect()
}

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

    TranscriptOutput::new(
        output.clone(),
        format!("codex-transcript:{}", short_hash(&output)),
    )
}

fn assistant_text_from_entry(entry: Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }

    let payload = entry.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("assistant")
    {
        return None;
    }

    if let Some(phase) = payload.get("phase").and_then(Value::as_str) {
        if phase != "final_answer" {
            return None;
        }
    }

    let content = payload.get("content")?;
    let text = match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("output_text") {
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
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"first"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"reasoning","text":"skip"}},{{"type":"output_text","text":"second"}},{{"type":"output_text","text":"answer"}}]}}}}"#
        )
        .unwrap();

        let result = read_last_assistant(file.path());
        assert_eq!(result.output, "second\nanswer");
        assert!(result.signature.unwrap().starts_with("codex-transcript:"));
    }

    #[test]
    fn skips_commentary_phase_and_returns_final_answer() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","phase":"commentary","content":[{{"type":"output_text","text":"thinking step one"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","phase":"final_answer","content":[{{"type":"output_text","text":"the answer"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","phase":"commentary","content":[{{"type":"output_text","text":"trailing chatter"}}]}}}}"#
        )
        .unwrap();

        let result = read_last_assistant(file.path());
        assert_eq!(result.output, "the answer");
    }
}
