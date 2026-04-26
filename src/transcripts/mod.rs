pub mod claude;
pub mod codex;

#[derive(Debug, Clone)]
pub struct TranscriptOutput {
    pub output: String,
    pub signature: Option<String>,
}

impl TranscriptOutput {
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
}
